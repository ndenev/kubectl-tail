mod cli;
mod kubernetes;
#[cfg(test)]
mod tests;
mod types;
mod utils;

use clap::Parser;
use crossterm::style::Stylize;
use futures::{TryStreamExt, stream::StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::runtime::watcher::{Config as WatcherConfig, Event, watcher};
use kube::{Api, Client, ResourceExt, config};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
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

    let mut label_selector_strings = Vec::<String>::new();
    let mut explicit_pods = std::collections::HashSet::new();

    for (typ, nam) in &resource_specs {
        if let Some(name) = nam {
            if let Some(sel) = get_selector_from_resource(&client, typ, name, &namespace).await? {
                if let Some(sel_str) = selector_to_labels_string(&sel) {
                    label_selector_strings.push(sel_str);
                } else if cli.verbose {
                    eprintln!(
                        "Selector for resource {}/{} is empty; skipping server-side filtering",
                        typ, name
                    );
                }
            } else if typ == "pod" {
                explicit_pods.insert(name.clone());
            }
        }
    }

    if let Some(sel_str) = &cli.selector {
        label_selector_strings.push(sel_str.clone());
    }

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

    // Track tail tasks per pod across watchers
    let handles: Arc<Mutex<HashMap<String, Vec<AbortHandle>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    for selector in label_selector_strings {
        let pods_api: Api<Pod> = Api::namespaced(client.clone(), &namespace);
        let handles = handles.clone();
        let tx = tx.clone();
        let namespace = namespace.clone();
        let container = cli.container.clone();
        let client = client.clone();
        let verbose = cli.verbose;
        tokio::spawn(async move {
            let cfg = WatcherConfig::default().labels(&selector);
            if verbose {
                eprintln!("Starting watcher for selector: {}", selector);
            }
            if let Err(err) = watch_pods(
                pods_api, cfg, handles, client, namespace, container, tx, cli.tail, verbose,
            )
            .await
                && verbose
            {
                eprintln!("Watcher with selector {} stopped: {}", selector, err);
            }
        });
    }

    for pod_name in explicit_pods {
        let pods_api: Api<Pod> = Api::namespaced(client.clone(), &namespace);
        let handles = handles.clone();
        let tx = tx.clone();
        let namespace = namespace.clone();
        let container = cli.container.clone();
        let client = client.clone();
        let verbose = cli.verbose;
        tokio::spawn(async move {
            let field_selector = format!("metadata.name={}", pod_name);
            let cfg = WatcherConfig::default().fields(&field_selector);
            if verbose {
                eprintln!("Starting watcher for pod: {}", pod_name);
            }
            if let Err(err) = watch_pods(
                pods_api, cfg, handles, client, namespace, container, tx, cli.tail, verbose,
            )
            .await
                && verbose
            {
                eprintln!("Watcher for pod {} stopped: {}", pod_name, err);
            }
        });
    }

    // Wait for interrupt signal to keep the program running
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn watch_pods(
    pods_api: Api<Pod>,
    cfg: WatcherConfig,
    handles: Arc<Mutex<HashMap<String, Vec<AbortHandle>>>>,
    client: Client,
    namespace: String,
    container: Option<String>,
    tx: mpsc::Sender<LogMessage>,
    tail: Option<i64>,
    verbose: bool,
) -> anyhow::Result<()> {
    let mut stream = watcher(pods_api, cfg).boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            Event::Apply(pod) | Event::InitApply(pod) => {
                handle_pod_event(
                    pod,
                    &handles,
                    client.clone(),
                    namespace.clone(),
                    container.clone(),
                    tx.clone(),
                    tail,
                    verbose,
                )
                .await;
            }
            Event::Delete(pod) => {
                let name = pod.name_any();
                let deletion_time = pod
                    .metadata
                    .deletion_timestamp
                    .as_ref()
                    .map(|t| t.0.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                    .unwrap_or_else(|| "now".to_string());

                eprintln!(
                    "🗑️  POD DELETED: {} | Deletion time: {}",
                    name, deletion_time
                );
                stop_tailing_pod(&name, &handles, verbose).await;
            }
            Event::Init => {
                eprintln!("🔄 Initializing pod watcher for namespace: {}", namespace);
            }
            Event::InitDone => {
                eprintln!(
                    "✅ Pod watcher initialization complete for namespace: {}",
                    namespace
                );
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_pod_event(
    pod: Pod,
    handles: &Arc<Mutex<HashMap<String, Vec<AbortHandle>>>>,
    client: Client,
    namespace: String,
    container: Option<String>,
    tx: mpsc::Sender<LogMessage>,
    tail: Option<i64>,
    verbose: bool,
) {
    let name = pod.name_any();
    let phase = pod
        .status
        .as_ref()
        .and_then(|s| s.phase.as_ref())
        .unwrap_or(&"Unknown".to_string())
        .clone();

    let is_running = phase == "Running";
    let is_terminating = pod.metadata.deletion_timestamp.is_some();

    // Check if this is a pod state change
    {
        let guard = handles.lock().await;
        let was_tracking = guard.contains_key(&name);

        if !was_tracking && (is_running || phase == "Pending") {
            // This is a new pod we haven't seen before
            // (Continue with new pod logging below)
        } else if was_tracking && !is_running {
            // Pod state changed from running to non-running
            if is_terminating {
                eprintln!(
                    "🟡 POD TERMINATING: {} | Phase: {} | Finalizing...",
                    name, phase
                );
            } else {
                eprintln!(
                    "⚠️  POD STATUS CHANGED: {} | Phase: {} | Stopped tailing",
                    name, phase
                );
            }
            drop(guard); // Release lock before calling stop_tailing_pod
            stop_tailing_pod(&name, handles, verbose).await;
            return;
        } else if was_tracking {
            // We're already tracking this running pod, no need to log again
            return;
        } else {
            // Pod exists but not running and we weren't tracking it
            if verbose {
                eprintln!("📋 POD STATUS: {} | Phase: {} | Not tailing", name, phase);
            }
            return;
        }
    }

    // This is a new pod - log detailed information
    let creation_time = pod
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| t.0.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let restart_count = pod
        .status
        .as_ref()
        .and_then(|s| s.container_statuses.as_ref())
        .map(|statuses| statuses.iter().map(|cs| cs.restart_count).sum::<i32>())
        .unwrap_or(0);

    let age = pod
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| {
            let now = chrono::Utc::now();
            let created = t.0;
            let duration = now.signed_duration_since(created);
            format_duration(duration)
        })
        .unwrap_or_else(|| "unknown".to_string());

    let node = pod
        .spec
        .as_ref()
        .and_then(|s| s.node_name.as_ref())
        .map(|n| n.as_str())
        .unwrap_or("unscheduled");

    if phase == "Pending" {
        eprintln!(
            "🟡 NEW POD: {} | Phase: {} | Age: {} | Node: {} | Created: {}",
            name, phase, age, node, creation_time
        );
    } else {
        eprintln!(
            "🔵 NEW POD: {} | Phase: {} | Age: {} | Restarts: {} | Node: {} | Created: {}",
            name, phase, age, restart_count, node, creation_time
        );
    }

    let pod_handles = spawn_tail_tasks_for_pod(
        client,
        name.clone(),
        namespace,
        container,
        tx,
        tail,
        verbose,
    )
    .await;

    let mut guard = handles.lock().await;
    guard.insert(name.clone(), pod_handles);
    if verbose {
        eprintln!("✅ Started tailing pod {}", name);
    }
}

async fn stop_tailing_pod(
    name: &str,
    handles: &Arc<Mutex<HashMap<String, Vec<AbortHandle>>>>,
    _verbose: bool,
) {
    let mut guard = handles.lock().await;
    if let Some(pod_handles) = guard.remove(name) {
        eprintln!("🔴 POD REMOVED: {} | Stopped tailing logs", name);
        for handle in pod_handles {
            handle.abort();
        }
    }
}

/// Format a duration in a human-readable way
fn format_duration(duration: chrono::Duration) -> String {
    let total_secs = duration.num_seconds();
    if total_secs < 60 {
        format!("{}s", total_secs)
    } else if total_secs < 3600 {
        format!("{}m{}s", total_secs / 60, total_secs % 60)
    } else if total_secs < 86400 {
        let hours = total_secs / 3600;
        let minutes = (total_secs % 3600) / 60;
        format!("{}h{}m", hours, minutes)
    } else {
        let days = total_secs / 86400;
        let hours = (total_secs % 86400) / 3600;
        format!("{}d{}h", days, hours)
    }
}
