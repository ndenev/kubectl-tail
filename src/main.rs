mod cli;
mod kubernetes;
mod types;

use clap::Parser;
use futures::stream::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    Api, Client, ResourceExt,
    api::{ListParams, WatchParams},
    config,
};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use cli::Cli;
use kubernetes::{get_selector_from_resource, spawn_tail_tasks_for_pod};
use types::LogMessage;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parsing_deployment() {
        let args = vec!["kubectl-tail", "deployment/my-deployment"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.resource, Some("deployment/my-deployment".to_string()));
        assert!(cli.selector.is_none());
    }

    #[test]
    fn test_cli_parsing_pod() {
        let args = vec!["kubectl-tail", "my-pod"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.resource, Some("my-pod".to_string()));
    }

    #[test]
    fn test_cli_parsing_labels() {
        let args = vec!["kubectl-tail", "-l", "app=nginx"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.selector, Some("app=nginx".to_string()));
    }

    #[test]
    fn test_cli_parsing_container() {
        let args = vec!["kubectl-tail", "my-pod", "-c", "app"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.container, Some("app".to_string()));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let config = if let Some(ctx) = &cli.context {
        config::Config::from_kubeconfig(&config::KubeConfigOptions {
            context: Some(ctx.clone()),
            ..Default::default()
        })
        .await?
    } else {
        config::Config::infer().await?
    };
    let client = Client::try_from(config)?;
    let namespace = cli.namespace.as_deref().unwrap_or("default");

    let (resource_type, resource_name) = if let Some(res) = &cli.resource {
        if let Some(slash_pos) = res.find('/') {
            let typ = res[..slash_pos].to_string();
            let nam = res[slash_pos + 1..].to_string();
            (typ, Some(nam))
        } else {
            ("pod".to_string(), Some(res.clone()))
        }
    } else {
        ("pod".to_string(), None)
    };

    let label_selector_from_resource = if let Some(name) = &resource_name {
        get_selector_from_resource(&client, &resource_type, name, namespace).await?
    } else {
        None
    };

    let label_selector = label_selector_from_resource.or(cli.selector);

    let pods_api: Api<Pod> = Api::namespaced(client.clone(), namespace);

    // Initial list of pods
    let mut pod_names: Vec<String> = if let Some(sel) = &label_selector {
        let lp = ListParams::default().labels(sel);
        let pods = pods_api.list(&lp).await?;
        pods.iter().map(|p| p.name_any()).collect()
    } else if resource_type == "pod" && resource_name.is_some() {
        vec![resource_name.clone().unwrap()]
    } else {
        let pods = pods_api.list(&ListParams::default()).await?;
        pods.iter().map(|p| p.name_any()).collect()
    };

    let lp = if let Some(sel) = &label_selector {
        ListParams::default().labels(sel)
    } else {
        ListParams::default()
    };

    println!("Found pods: {:?}", pod_names);

    // Channel for log messages
    let (tx, mut rx) = mpsc::channel::<LogMessage>(100);

    // Spawn task to print logs
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            println!("[{}/{}] {}", msg.pod_name, msg.container_name, msg.line);
        }
    });

    // HashMap to store abort handles per pod (multiple per pod for multi-container)
    let mut handles: HashMap<String, Vec<AbortHandle>> = HashMap::new();

    // Spawn tail tasks for initial pods
    for name in &pod_names {
        let pod_handles = spawn_tail_tasks_for_pod(
            client.clone(),
            name.clone(),
            namespace.to_string(),
            cli.container.clone(),
            tx.clone(),
        )
        .await;
        handles.insert(name.clone(), pod_handles);
    }

    // Start watching for pod changes
    let wp = WatchParams::default().labels(&lp.label_selector.unwrap_or_default());
    let mut watcher = pods_api.watch(&wp, "0").await?.boxed();

    while let Some(event) = watcher.next().await {
        match event {
            Ok(kube::api::WatchEvent::Added(pod)) => {
                let name = pod.name_any();
                if !pod_names.contains(&name) {
                    pod_names.push(name.clone());
                    println!("New pod added: {}", name);
                    let pod_handles = spawn_tail_tasks_for_pod(
                        client.clone(),
                        name.clone(),
                        namespace.to_string(),
                        cli.container.clone(),
                        tx.clone(),
                    )
                    .await;
                    handles.insert(name.clone(), pod_handles);
                }
            }
            Ok(kube::api::WatchEvent::Deleted(pod)) => {
                let name = pod.name_any();
                pod_names.retain(|n| n != &name);
                println!("Pod deleted: {}", name);
                if let Some(pod_handles) = handles.remove(&name) {
                    for handle in pod_handles {
                        handle.abort();
                    }
                }
            }
            Ok(kube::api::WatchEvent::Modified(_pod)) => {
                // Handle modifications if needed, e.g., phase change
            }
            Err(e) => {
                eprintln!("Watch error: {}", e);
                break;
            }
            _ => {}
        }
    }

    Ok(())
}
