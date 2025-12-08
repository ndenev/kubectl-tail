mod cli;
mod kubernetes;
mod types;

use clap::Parser;
use kube::{Api, Client, api::{ListParams, WatchParams}, ResourceExt, config};
use k8s_openapi::api::core::v1::Pod;
use futures::stream::StreamExt;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use cli::Cli;
use kubernetes::{get_selector_from_resource, spawn_tail_tasks_for_pod};
use types::LogMessage;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let config = if let Some(ctx) = &cli.context {
        config::Config::from_kubeconfig(&config::KubeConfigOptions {
            context: Some(ctx.clone()),
            ..Default::default()
        }).await?
    } else {
        config::Config::infer().await?
    };
    let client = Client::try_from(config)?;
    let namespace = cli.namespace.as_deref().unwrap_or("default");

    let mut label_selector = cli.selector;

    if let Some(name) = &cli.resource_name {
        // Get selector from the resource
        label_selector = Some(get_selector_from_resource(&client, &cli.resource_type, name, namespace).await?);
    }

    let lp = if let Some(sel) = &label_selector {
        ListParams::default().labels(sel)
    } else {
        ListParams::default()
    };

    let pods_api: Api<Pod> = Api::namespaced(client.clone(), namespace);

    // Initial list of pods
    let pods = pods_api.list(&lp).await?;
    let mut pod_names: Vec<String> = pods.iter().map(|p| p.name_any()).collect();

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
        let pod_handles = spawn_tail_tasks_for_pod(client.clone(), name.clone(), namespace.to_string(), cli.container.clone(), tx.clone()).await;
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
                    let pod_handles = spawn_tail_tasks_for_pod(client.clone(), name.clone(), namespace.to_string(), cli.container.clone(), tx.clone()).await;
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
