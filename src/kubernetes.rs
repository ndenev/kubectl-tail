use crate::types::LogMessage;
use kube::{Api, Client, api::{LogParams}};
use k8s_openapi::api::core::v1::Pod;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

pub async fn get_selector_from_resource(client: &Client, resource_type: &str, name: &str, namespace: &str) -> anyhow::Result<String> {
    match resource_type {
        "deployment" => {
            use k8s_openapi::api::apps::v1::Deployment;
            let api: Api<Deployment> = Api::namespaced(client.clone(), namespace);
            let dep = api.get(name).await?;
            let spec = dep.spec.ok_or_else(|| anyhow::anyhow!("No spec for deployment"))?;
            let match_labels = spec.selector.match_labels.ok_or_else(|| anyhow::anyhow!("No match_labels"))?;
            let labels: Vec<String> = match_labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            Ok(labels.join(","))
        }
        "statefulset" => {
            use k8s_openapi::api::apps::v1::StatefulSet;
            let api: Api<StatefulSet> = Api::namespaced(client.clone(), namespace);
            let sts = api.get(name).await?;
            let spec = sts.spec.ok_or_else(|| anyhow::anyhow!("No spec for statefulset"))?;
            let match_labels = spec.selector.match_labels.ok_or_else(|| anyhow::anyhow!("No match_labels"))?;
            let labels: Vec<String> = match_labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            Ok(labels.join(","))
        }
        "daemonset" => {
            use k8s_openapi::api::apps::v1::DaemonSet;
            let api: Api<DaemonSet> = Api::namespaced(client.clone(), namespace);
            let ds = api.get(name).await?;
            let spec = ds.spec.ok_or_else(|| anyhow::anyhow!("No spec for daemonset"))?;
            let match_labels = spec.selector.match_labels.ok_or_else(|| anyhow::anyhow!("No match_labels"))?;
            let labels: Vec<String> = match_labels.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
            Ok(labels.join(","))
        }
        "pod" => {
            anyhow::bail!("For pod, use label selector or specify labels");
        }
        _ => anyhow::bail!("Unsupported resource type: {}", resource_type),
    }
}

pub async fn spawn_tail_tasks_for_pod(client: Client, pod_name: String, namespace: String, container: Option<String>, tx: mpsc::Sender<LogMessage>) -> Vec<AbortHandle> {
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
                        let handle = spawn_tail_task(client.clone(), pod_name.clone(), namespace.clone(), c.name.clone(), tx.clone());
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

pub fn spawn_tail_task(client: Client, pod_name: String, namespace: String, container_name: String, tx: mpsc::Sender<LogMessage>) -> AbortHandle {
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