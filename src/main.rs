mod cli;
mod kubernetes;
#[cfg(test)]
mod tests;
mod types;
mod utils;

use clap::Parser;
use crossterm::style::Stylize;
use futures::stream::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::{
    Api, Client, ResourceExt,
    api::{ListParams, WatchParams},
    config,
};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use cli::Cli;
use kubernetes::{get_selector_from_resource, spawn_tail_tasks_for_pod};
use types::LogMessage;
use utils::*;

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
    let namespace = cli.namespace.as_deref().unwrap_or("default").to_string();

    let pods_api: Api<Pod> = Api::namespaced(client.clone(), &namespace);

    if cli.resources.is_empty() && cli.selector.is_none() {
        eprintln!("Must specify at least one resource or a label selector (--selector)");
        std::process::exit(1);
    }

    let mut resource_specs = vec![];
    for res in &cli.resources {
        let (typ, nam) = if let Some(slash_pos) = res.find('/') {
            let typ = res[..slash_pos].to_string();
            let nam = res[slash_pos + 1..].to_string();
            (typ, Some(nam))
        } else {
            ("pod".to_string(), Some(res.clone()))
        };
        resource_specs.push((typ, nam));
    }

    let mut all_selectors = Vec::<LabelSelector>::new();
    let mut initial_pods = std::collections::HashSet::new();

    for (typ, nam) in &resource_specs {
        if let Some(name) = nam {
            if let Some(sel) = get_selector_from_resource(&client, typ, name, &namespace).await? {
                all_selectors.push(sel.clone());
                let labels_str = selector_to_labels_string(&sel);
                let lp = if let Some(s) = labels_str {
                    ListParams::default().labels(&s)
                } else {
                    ListParams::default()
                };
                let pods = pods_api.list(&lp).await?;
                for p in pods {
                    if p.status.as_ref().and_then(|s| s.phase.as_ref())
                        == Some(&"Running".to_string())
                    {
                        initial_pods.insert(p.name_any());
                    }
                }
            } else if typ == "pod" {
                initial_pods.insert(name.clone());
            }
        }
    }

    if let Some(sel_str) = &cli.selector {
        let match_labels = parse_labels(sel_str);
        let selector = LabelSelector {
            match_labels: Some(match_labels),
            match_expressions: None,
        };
        all_selectors.push(selector.clone());
        let lp = ListParams::default().labels(sel_str);
        let pods = pods_api.list(&lp).await?;
        for p in pods {
            if p.status.as_ref().and_then(|s| s.phase.as_ref()) == Some(&"Running".to_string()) {
                initial_pods.insert(p.name_any());
            }
        }
    }

    let mut pod_names: Vec<String> = initial_pods.into_iter().collect();

    println!("Found pods: {:?}", pod_names);

    // Channel for log messages
    let (tx, mut rx) = mpsc::channel::<LogMessage>(100);

    // Spawn task to print logs
    tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let color = get_color(&msg.pod_name);
            let prefix = format!("[{}/{}]", msg.pod_name, msg.container_name).with(color);
            println!("{} {}", prefix, msg.line);
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
            cli.tail,
            cli.verbose,
        )
        .await;
        handles.insert(name.clone(), pod_handles);
    }

    // Start watching for pod changes
    let all_selectors_clone = all_selectors.clone();
    let verbose = cli.verbose;
    tokio::spawn(async move {
        let wp = WatchParams::default();
        loop {
            match pods_api.watch(&wp, "0").await {
                Ok(w) => {
                    let mut watcher = w.boxed();
                    while let Some(event) = watcher.next().await {
                        match event {
                            Ok(kube::api::WatchEvent::Added(pod)) => {
                                let name = pod.name_any();
                                if pod.status.as_ref().and_then(|s| s.phase.as_ref())
                                    == Some(&"Running".to_string())
                                    && !pod_names.contains(&name)
                                {
                                    let labels = pod.metadata.labels.as_ref();
                                    if !all_selectors_clone.is_empty()
                                        && labels.is_some_and(|l| {
                                            all_selectors_clone
                                                .iter()
                                                .any(|sel| matches_selector(l, sel))
                                        })
                                    {
                                        pod_names.push(name.clone());
                                        if verbose {
                                            println!("New pod added: {}", name);
                                        }
                                        let pod_handles = spawn_tail_tasks_for_pod(
                                            client.clone(),
                                            name.clone(),
                                            namespace.to_string(),
                                            cli.container.clone(),
                                            tx.clone(),
                                            cli.tail,
                                            verbose,
                                        )
                                        .await;
                                        handles.insert(name.clone(), pod_handles);
                                    }
                                }
                            }
                            Ok(kube::api::WatchEvent::Deleted(pod)) => {
                                let name = pod.name_any();
                                pod_names.retain(|n| n != &name);
                                if verbose {
                                    println!("Pod deleted: {}", name);
                                }
                                if let Some(pod_handles) = handles.remove(&name) {
                                    for handle in pod_handles {
                                        handle.abort();
                                    }
                                }
                            }
                            Ok(kube::api::WatchEvent::Modified(pod)) => {
                                let name = pod.name_any();
                                let is_running = pod.status.as_ref().and_then(|s| s.phase.as_ref())
                                    == Some(&"Running".to_string());
                                let is_in_list = pod_names.contains(&name);
                                if is_running && !is_in_list {
                                    let labels = pod.metadata.labels.as_ref();
                                    if !all_selectors_clone.is_empty()
                                        && labels.is_some_and(|l| {
                                            all_selectors_clone
                                                .iter()
                                                .any(|sel| matches_selector(l, sel))
                                        })
                                    {
                                        pod_names.push(name.clone());
                                        if verbose {
                                            println!("Pod became running: {}", name);
                                        }
                                        let pod_handles = spawn_tail_tasks_for_pod(
                                            client.clone(),
                                            name.clone(),
                                            namespace.to_string(),
                                            cli.container.clone(),
                                            tx.clone(),
                                            cli.tail,
                                            verbose,
                                        )
                                        .await;
                                        handles.insert(name.clone(), pod_handles);
                                    }
                                } else if !is_running && is_in_list {
                                    pod_names.retain(|n| n != &name);
                                    if verbose {
                                        println!("Pod stopped running: {}", name);
                                    }
                                    if let Some(pod_handles) = handles.remove(&name) {
                                        for handle in pod_handles {
                                            handle.abort();
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                if verbose {
                                    eprintln!("Watch error: {}, retrying", e);
                                }
                                break;
                            }
                            _ => {}
                        }
                    }
                    if verbose {
                        eprintln!("Watch stream ended, retrying in 5 seconds");
                    }
                }
                Err(e) => {
                    if verbose {
                        eprintln!("Failed to start pod watch: {}, retrying in 5 seconds", e);
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Wait for interrupt signal to keep the program running
    tokio::signal::ctrl_c().await?;
    Ok(())
}
