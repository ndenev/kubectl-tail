use crate::types::{LogMessage, LogEvent, GapReason, ConnectionState};
use futures::io::AsyncBufReadExt;
use futures::stream::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::{Api, Client, api::LogParams};
use std::fmt::Debug;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use std::time::{Duration, SystemTime};

trait HasSelector {
    fn get_selector(&self) -> Option<&LabelSelector>;
}

impl HasSelector for k8s_openapi::api::apps::v1::Deployment {
    fn get_selector(&self) -> Option<&LabelSelector> {
        self.spec.as_ref().map(|s| &s.selector)
    }
}

impl HasSelector for k8s_openapi::api::apps::v1::StatefulSet {
    fn get_selector(&self) -> Option<&LabelSelector> {
        self.spec.as_ref().map(|s| &s.selector)
    }
}

impl HasSelector for k8s_openapi::api::apps::v1::DaemonSet {
    fn get_selector(&self) -> Option<&LabelSelector> {
        self.spec.as_ref().map(|s| &s.selector)
    }
}

impl HasSelector for k8s_openapi::api::apps::v1::ReplicaSet {
    fn get_selector(&self) -> Option<&LabelSelector> {
        self.spec.as_ref().map(|s| &s.selector)
    }
}

impl HasSelector for k8s_openapi::api::batch::v1::Job {
    fn get_selector(&self) -> Option<&LabelSelector> {
        self.spec.as_ref().and_then(|s| s.selector.as_ref())
    }
}

async fn get_selector_from_resource_generic<T>(
    client: &Client,
    name: &str,
    namespace: &str,
) -> anyhow::Result<Option<LabelSelector>>
where
    T: k8s_openapi::Resource<Scope = k8s_openapi::NamespaceResourceScope>
        + k8s_openapi::Metadata<Ty = k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta>
        + HasSelector
        + serde::de::DeserializeOwned
        + serde::Serialize
        + Clone
        + Debug
        + Send
        + Sync,
{
    let api: Api<T> = Api::namespaced(client.clone(), namespace);
    let res = api.get(name).await?;
    let selector = res
        .get_selector()
        .ok_or_else(|| anyhow::anyhow!("No selector"))?;
    Ok(Some(selector.clone()))
}

pub async fn get_selector_from_resource(
    client: &Client,
    resource_type: &str,
    name: &str,
    namespace: &str,
) -> anyhow::Result<Option<LabelSelector>> {
    match resource_type {
        "deployment" => {
            get_selector_from_resource_generic::<k8s_openapi::api::apps::v1::Deployment>(
                client, name, namespace,
            )
            .await
        }
        "statefulset" => {
            get_selector_from_resource_generic::<k8s_openapi::api::apps::v1::StatefulSet>(
                client, name, namespace,
            )
            .await
        }
        "daemonset" => {
            get_selector_from_resource_generic::<k8s_openapi::api::apps::v1::DaemonSet>(
                client, name, namespace,
            )
            .await
        }
        "job" => {
            get_selector_from_resource_generic::<k8s_openapi::api::batch::v1::Job>(
                client, name, namespace,
            )
            .await
        }
        "replicaset" => {
            get_selector_from_resource_generic::<k8s_openapi::api::apps::v1::ReplicaSet>(
                client, name, namespace,
            )
            .await
        }
        "pod" => Ok(None),
        _ => anyhow::bail!("Unsupported resource type: {}", resource_type),
    }
}

pub async fn spawn_tail_tasks_for_pod(
    client: Client,
    pod_name: String,
    namespace: String,
    container: Option<String>,
    tx: mpsc::Sender<LogEvent>,
    tail: Option<i64>,
    verbose: bool,
) -> Vec<AbortHandle> {
    if let Some(cont) = container {
        vec![spawn_tail_task(
            client, pod_name, namespace, cont, tx, tail, verbose,
        )]
    } else {
        // Fetch pod to get container names
        let api: Api<Pod> = Api::namespaced(client.clone(), &namespace);
        match api.get(&pod_name).await {
            Ok(pod) => {
                let mut handles = Vec::new();
                if let Some(spec) = &pod.spec {
                    for c in &spec.containers {
                        let handle = spawn_tail_task(
                            client.clone(),
                            pod_name.clone(),
                            namespace.clone(),
                            c.name.clone(),
                            tx.clone(),
                            tail,
                            verbose,
                        );
                        handles.push(handle);
                    }
                }
                handles
            }
            Err(e) => {
                if verbose {
                    eprintln!("Failed to get pod {} for containers: {}", pod_name, e);
                }
                vec![]
            }
        }
    }
}

pub fn spawn_tail_task(
    client: Client,
    pod_name: String,
    namespace: String,
    container_name: String,
    tx: mpsc::Sender<LogEvent>,
    tail: Option<i64>,
    verbose: bool,
) -> AbortHandle {
    let api: Api<Pod> = Api::namespaced(client, &namespace);

    let handle = tokio::spawn(async move {
        if verbose {
            eprintln!(
                "Starting to tail logs for pod {}/{} in namespace {}",
                pod_name, container_name, namespace
            );
        }
        let mut is_first_attempt = true;
        let mut reconnection_attempt = 0;
        let mut disconnection_time: Option<SystemTime> = None;

        loop {
            // Follow logs with tail only on first attempt to avoid replay
            let tail_lines = if is_first_attempt { tail } else { None };
            is_first_attempt = false;
            reconnection_attempt += 1;
            let lp_follow = LogParams {
                follow: true,
                container: Some(container_name.clone()),
                tail_lines,
                ..Default::default()
            };
            match api.log_stream(&pod_name, &lp_follow).await {
                Ok(stream) => {
                    // Connection successful - emit status change and reset counters
                    if reconnection_attempt > 1 {
                        if let Some(disconnect_time) = disconnection_time.take() {
                            let downtime = SystemTime::now().duration_since(disconnect_time).unwrap_or_default();
                            let _ = tx.send(LogEvent::Gap {
                                duration: downtime,
                                reason: GapReason::StreamEnded,
                                pod: pod_name.clone(),
                                container: container_name.clone(),
                            }).await;
                        }

                        let _ = tx.send(LogEvent::StatusChange {
                            pod: pod_name.clone(),
                            container: container_name.clone(),
                            old_state: ConnectionState::Reconnecting { attempt: reconnection_attempt - 1 },
                            new_state: ConnectionState::Connected,
                        }).await;
                    }

                    reconnection_attempt = 0;
                    let mut line_stream = stream.lines();

                    while let Some(line_result) = line_stream.next().await {
                        match line_result {
                            Ok(line) => {
                                let msg = LogMessage::new(
                                    pod_name.clone(),
                                    container_name.clone(),
                                    line,
                                );
                                if tx.send(LogEvent::Log(msg)).await.is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                if verbose {
                                    eprintln!(
                                        "Error reading follow log line from pod {}/{}: {}, retrying",
                                        pod_name, container_name, e
                                    );
                                }
                                break;
                            }
                        }
                    }
                    // If stream ended, mark disconnection and retry
                    disconnection_time = Some(SystemTime::now());
                    let _ = tx.send(LogEvent::StatusChange {
                        pod: pod_name.clone(),
                        container: container_name.clone(),
                        old_state: ConnectionState::Connected,
                        new_state: ConnectionState::Reconnecting { attempt: 1 },
                    }).await;

                    if verbose {
                        eprintln!(
                            "Log stream ended for pod {}/{}, retrying in 5 seconds",
                            pod_name, container_name
                        );
                    }
                }
                Err(e) => {
                    // Mark disconnection if first error
                    if disconnection_time.is_none() {
                        disconnection_time = Some(SystemTime::now());
                    }

                    let _gap_reason = if let kube::Error::Api(err) = &e {
                        if err.code == 404 {
                            let _ = tx.send(LogEvent::StatusChange {
                                pod: pod_name.clone(),
                                container: container_name.clone(),
                                old_state: ConnectionState::Connected,
                                new_state: ConnectionState::Failed("Pod not found".to_string()),
                            }).await;

                            if verbose {
                                eprintln!(
                                    "Pod {}/{} not found (404), stopping tail",
                                    pod_name, container_name
                                );
                            }
                            return;
                        }
                        GapReason::ApiServerError(err.code)
                    } else {
                        GapReason::NetworkError(e.to_string())
                    };

                    let _ = tx.send(LogEvent::StatusChange {
                        pod: pod_name.clone(),
                        container: container_name.clone(),
                        old_state: ConnectionState::Connected,
                        new_state: ConnectionState::Reconnecting { attempt: reconnection_attempt },
                    }).await;

                    if verbose {
                        eprintln!(
                            "Failed to get follow log stream for pod {}/{}: {}, retrying in 5 seconds",
                            pod_name, container_name, e
                        );
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    handle.abort_handle()
}
