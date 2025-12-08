use crate::types::LogMessage;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::{Api, Client, api::LogParams};
use std::collections::BTreeMap;
use std::fmt::Debug;
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
) -> Vec<AbortHandle> {
    if let Some(cont) = container {
        vec![spawn_tail_task(client, pod_name, namespace, cont, tx)]
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
) -> AbortHandle {
    let api: Api<Pod> = Api::namespaced(client, &namespace);

    let handle = tokio::spawn(async move {
        let lp = LogParams {
            follow: true,
            container: Some(container_name.clone()),
            ..Default::default()
        };

        match api.logs(&pod_name, &lp).await {
            Ok(logs) => {
                for line in logs.lines() {
                    let msg = LogMessage {
                        pod_name: pod_name.clone(),
                        container_name: container_name.clone(),
                        line: line.to_string(),
                    };
                    if tx.send(msg).await.is_err() {
                        break;
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to get logs for {}: {}", pod_name, e);
            }
        }
    });

    handle.abort_handle()
}
