use crate::types::LogMessage;
use futures::io::AsyncBufReadExt;
use futures::stream::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::{Api, Client, api::LogParams};
use std::fmt::Debug;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tracing::{debug, warn};

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
    cluster: String,
    pod_name: String,
    namespace: String,
    container: Option<String>,
    tx: mpsc::Sender<LogMessage>,
    tail: Option<i64>,
) -> Vec<AbortHandle> {
    if let Some(cont) = container {
        vec![spawn_tail_task(
            client, cluster, pod_name, namespace, cont, tx, tail,
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
                            cluster.clone(),
                            pod_name.clone(),
                            namespace.clone(),
                            c.name.clone(),
                            tx.clone(),
                            tail,
                        );
                        handles.push(handle);
                    }
                }
                handles
            }
            Err(e) => {
                debug!("Failed to get pod {} for containers: {}", pod_name, e);
                vec![]
            }
        }
    }
}

pub fn spawn_tail_task(
    client: Client,
    cluster: String,
    pod_name: String,
    namespace: String,
    container_name: String,
    tx: mpsc::Sender<LogMessage>,
    tail: Option<i64>,
) -> AbortHandle {
    let api: Api<Pod> = Api::namespaced(client, &namespace);

    let handle = tokio::spawn(async move {
        debug!(
            "Starting to tail logs for pod {}/{} in namespace {}",
            pod_name, container_name, namespace
        );
        let mut is_first_attempt = true;
        let mut last_log_time: Option<chrono::DateTime<chrono::Utc>> = None;
        let mut recent_logs: std::collections::VecDeque<String> =
            std::collections::VecDeque::with_capacity(100);

        loop {
            let is_reconnection = !is_first_attempt;
            let lp_follow = if is_first_attempt {
                // First attempt: use user-specified tail
                is_first_attempt = false;
                LogParams {
                    follow: true,
                    container: Some(container_name.clone()),
                    tail_lines: tail,
                    ..Default::default()
                }
            } else {
                // Reconnection: use sinceTime to avoid replay
                if let Some(since_time) = last_log_time {
                    // Add 1 second to avoid getting the exact same log line again
                    let since_time_plus = since_time + chrono::Duration::seconds(1);
                    LogParams {
                        follow: true,
                        container: Some(container_name.clone()),
                        since_time: Some(since_time_plus),
                        ..Default::default()
                    }
                } else {
                    // Fallback: no tail to minimize replay
                    LogParams {
                        follow: true,
                        container: Some(container_name.clone()),
                        tail_lines: Some(0), // Get no historical logs on reconnect
                        ..Default::default()
                    }
                }
            };
            match api.log_stream(&pod_name, &lp_follow).await {
                Ok(stream) => {
                    if is_reconnection {
                        if last_log_time.is_some() {
                            debug!(
                                "Reconnected to {}/{} using sinceTime (no replay)",
                                pod_name, container_name
                            );
                        } else {
                            debug!(
                                "Reconnected to {}/{} with no tail (minimal replay)",
                                pod_name, container_name
                            );
                        }
                    }
                    let mut line_stream = stream.lines();
                    while let Some(line_result) = line_stream.next().await {
                        match line_result {
                            Ok(line) => {
                                // Simple deduplication: skip if we've seen this exact log line recently
                                if is_reconnection && recent_logs.contains(&line) {
                                    debug!(
                                        "Skipping duplicate log line on reconnection for {}/{}",
                                        pod_name, container_name
                                    );
                                    continue;
                                }

                                // Track recent log lines for deduplication (keep last 100)
                                recent_logs.push_back(line.clone());
                                if recent_logs.len() > 100 {
                                    recent_logs.pop_front();
                                }

                                // Update last log time to current time for reconnection purposes
                                last_log_time = Some(chrono::Utc::now());

                                let msg = LogMessage {
                                    cluster: cluster.clone(),
                                    namespace: namespace.clone(),
                                    pod_name: pod_name.clone(),
                                    container_name: container_name.clone(),
                                    line,
                                    timestamp: chrono::Utc::now(),
                                };
                                if tx.send(msg).await.is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "Error reading follow log line from pod {}/{}: {}, retrying",
                                    pod_name, container_name, e
                                );
                                break;
                            }
                        }
                    }
                    // If stream ended, retry
                    debug!(
                        "Log stream ended for pod {}/{}, retrying in 5 seconds",
                        pod_name, container_name
                    );
                }
                Err(e) => {
                    if let kube::Error::Api(err) = &e
                        && err.code == 404
                    {
                        debug!(
                            "Pod {}/{} not found (404), stopping tail",
                            pod_name, container_name
                        );
                        return;
                    }
                    warn!(
                        "Failed to get follow log stream for pod {}/{}: {}, retrying in 5 seconds",
                        pod_name, container_name, e
                    );
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    handle.abort_handle()
}
