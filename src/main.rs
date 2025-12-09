mod cli;
mod kubernetes;
#[cfg(test)]
mod tests;
mod types;
mod ui;
mod utils;

use clap::Parser;
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
use types::LogEvent;
use utils::*;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Use TUI only when explicitly requested and output is a terminal
    let use_tui = cli.tui && atty::is(atty::Stream::Stdout);

    if use_tui {
        run_tui_mode(cli).await
    } else {
        run_plain_mode(cli).await
    }
}

async fn run_plain_mode(cli: Cli) -> anyhow::Result<()> {

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

    // Channel for log events
    let (tx, mut rx) = mpsc::channel::<LogEvent>(1000);

    // Spawn task to print logs in plain mode
    let verbose = cli.verbose;
    tokio::spawn(async move {
        use std::io::{self, Write};
        while let Some(event) = rx.recv().await {
            match event {
                LogEvent::Log(msg) => {
                    // Use only stdout and avoid color codes in plain mode
                    let prefix = format!("[{}/{}]", msg.pod_name, msg.container_name);
                    println!("{} {}", prefix, msg.line);
                    let _ = io::stdout().flush();
                },
                LogEvent::Gap { duration, reason, pod, container } => {
                    if verbose {
                        println!("WARNING: LOG GAP: {}/{} missing {:.1}s of logs ({:?})",
                                pod, container, duration.as_secs_f32(), reason);
                        let _ = io::stdout().flush();
                    }
                },
                LogEvent::StatusChange { pod, container, new_state, .. } => {
                    if verbose {
                        println!("STATUS: {}/{} is now {:?}", pod, container, new_state);
                        let _ = io::stdout().flush();
                    }
                },
                LogEvent::SystemMessage { message, .. } => {
                    if verbose {
                        println!("INFO: {}", message);
                        let _ = io::stdout().flush();
                    }
                },
            }
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
            .await && verbose
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
            .await && verbose
            {
                eprintln!("Watcher for pod {} stopped: {}", pod_name, err);
            }
        });
    }

    // Wait for interrupt signal to keep the program running
    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn run_tui_mode(cli: Cli) -> anyhow::Result<()> {
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

    // Channel for log events
    let (tx, rx) = mpsc::channel::<LogEvent>(cli.buffer_size);

    // Track tail tasks per pod across watchers
    let handles: Arc<Mutex<HashMap<String, Vec<AbortHandle>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Start watchers for label selectors
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
            .await && verbose
            {
                eprintln!("Watcher with selector {} stopped: {}", selector, err);
            }
        });
    }

    // Start watchers for explicit pods
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
            .await && verbose
            {
                eprintln!("Watcher for pod {} stopped: {}", pod_name, err);
            }
        });
    }

    // Start TUI
    ui::run_tui(cli.buffer_size, cli.auto_scroll, rx).await
}

#[allow(clippy::too_many_arguments)]
async fn watch_pods(
    pods_api: Api<Pod>,
    cfg: WatcherConfig,
    handles: Arc<Mutex<HashMap<String, Vec<AbortHandle>>>>,
    client: Client,
    namespace: String,
    container: Option<String>,
    tx: mpsc::Sender<LogEvent>,
    tail: Option<i64>,
    verbose: bool,
) -> anyhow::Result<()> {
    let mut stream = watcher(pods_api, cfg).boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            Event::Apply(pod) | Event::InitApply(pod) => {
                handle_running_pod(
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
            Event::Delete(pod) => stop_tailing_pod(&pod.name_any(), &handles, verbose).await,
            Event::Init | Event::InitDone => {}
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_running_pod(
    pod: Pod,
    handles: &Arc<Mutex<HashMap<String, Vec<AbortHandle>>>>,
    client: Client,
    namespace: String,
    container: Option<String>,
    tx: mpsc::Sender<LogEvent>,
    tail: Option<i64>,
    verbose: bool,
) {
    let name = pod.name_any();
    let is_running = pod
        .status
        .as_ref()
        .and_then(|s| s.phase.as_ref())
        .map(|p| p == "Running")
        .unwrap_or(false);

    if !is_running {
        stop_tailing_pod(&name, handles, verbose).await;
        return;
    }

    {
        let guard = handles.lock().await;
        if guard.contains_key(&name) {
            return;
        }
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
        eprintln!("Started tailing pod {}", name);
    }
}

async fn stop_tailing_pod(
    name: &str,
    handles: &Arc<Mutex<HashMap<String, Vec<AbortHandle>>>>,
    verbose: bool,
) {
    let mut guard = handles.lock().await;
    if let Some(pod_handles) = guard.remove(name) {
        if verbose {
            eprintln!("Stopping tail for pod {}", name);
        }
        for handle in pod_handles {
            handle.abort();
        }
    }
}
