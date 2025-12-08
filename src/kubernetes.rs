use crate::types::LogMessage;
use futures::io::AsyncBufReadExt;
use futures::stream::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::{Api, Client, api::LogParams};
use std::collections::BTreeMap;
use std::fmt::Debug;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

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

fn extract_labels(match_labels: &BTreeMap<String, String>) -> String {
    match_labels
        .iter()
        .map(|(k, v)| format!("{}={}", k, v))
        .collect::<Vec<_>>()
        .join(",")
}

async fn get_selector_from_resource_generic<T>(
    client: &Client,
    name: &str,
    namespace: &str,
) -> anyhow::Result<Option<String>>
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
    let match_labels = selector
        .match_labels
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No match_labels"))?;
    Ok(Some(extract_labels(match_labels)))
}

pub async fn get_selector_from_resource(
    client: &Client,
    resource_type: &str,
    name: &str,
    namespace: &str,
) -> anyhow::Result<Option<String>> {
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
    tx: mpsc::Sender<LogMessage>,
    tail: Option<i64>,
) -> Vec<AbortHandle> {
    if let Some(cont) = container {
        vec![spawn_tail_task(client, pod_name, namespace, cont, tx, tail)]
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
                        );
                        handles.push(handle);
                    }
                }
                handles
            }
            Err(e) => {
                eprintln!("Failed to get pod {} for containers: {}", pod_name, e);
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
    tx: mpsc::Sender<LogMessage>,
    tail: Option<i64>,
) -> AbortHandle {
    let api: Api<Pod> = Api::namespaced(client, &namespace);

    let handle = tokio::spawn(async move {
        let _lp = LogParams {
            follow: true,
            container: Some(container_name.clone()),
            tail_lines: None,
            ..Default::default()
        };

        eprintln!(
            "Starting to tail logs for pod {}/{} in namespace {}",
            pod_name, container_name, namespace
        );
        // Follow logs with tail
        let lp_follow = LogParams {
            follow: true,
            container: Some(container_name.clone()),
            tail_lines: tail,
            ..Default::default()
        };
        loop {
            match api.log_stream(&pod_name, &lp_follow).await {
                Ok(stream) => {
                    let mut line_stream = stream.lines();
                    while let Some(line_result) = line_stream.next().await {
                        match line_result {
                            Ok(line) => {
                                let msg = LogMessage {
                                    pod_name: pod_name.clone(),
                                    container_name: container_name.clone(),
                                    line,
                                };
                                if tx.send(msg).await.is_err() {
                                    return;
                                }
                            }
                            Err(e) => {
                                eprintln!(
                                    "Error reading follow log line from pod {}/{}: {}, retrying",
                                    pod_name, container_name, e
                                );
                                break;
                            }
                        }
                    }
                    // If stream ended, retry
                    eprintln!(
                        "Log stream ended for pod {}/{}, retrying in 5 seconds",
                        pod_name, container_name
                    );
                }
                Err(e) => {
                    if let kube::Error::Api(err) = &e {
                        if err.code == 404 {
                            eprintln!(
                                "Pod {}/{} not found (404), stopping tail",
                                pod_name, container_name
                            );
                            return;
                        }
                    }
                    eprintln!(
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
