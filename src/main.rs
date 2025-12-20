mod cli;
mod kubernetes;
#[cfg(test)]
mod tests;
mod types;
mod ui;
mod utils;

use clap::Parser;
use crossterm::{
    execute,
    style::Stylize,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::{TryStreamExt, stream::StreamExt};
use k8s_openapi::api::core::v1::Pod;
use kube::runtime::watcher::{Config as WatcherConfig, Event, watcher};
use kube::{Api, Client, ResourceExt, config};
use ratatui::{Terminal, backend::CrosstermBackend};
use regex::Regex;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio::task::AbortHandle;
use tracing::{debug, error, info, warn};

use cli::Cli;
use kubernetes::{get_selector_from_resource, spawn_tail_tasks_for_pod};
use types::LogMessage;
use ui::app::{PodInfo, PodKey};
use ui::{App, AppEvent};
use utils::*;

/// Context for pod log tailing operations
#[derive(Clone)]
struct TailContext {
    client: Client,
    cluster: String,
    namespace: String,
    container: Option<String>,
    tx: mpsc::Sender<LogMessage>,
    tail: Option<i64>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Determine if we'll use TUI mode (needed to configure logging appropriately)
    let use_tui = !cli.no_tui && std::io::stdout().is_terminal();

    // Initialize tracing subscriber - configure differently for TUI vs stdout mode
    let filter = if cli.verbose { "debug" } else { "info" };
    if use_tui {
        // In TUI mode: write logs to a file to avoid corrupting the display
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/kubectl-tail.log")
            .unwrap_or_else(|_| {
                eprintln!("Warning: Could not open /tmp/kubectl-tail.log for logging");
                std::fs::File::create("/dev/null").expect("Failed to open /dev/null")
            });

        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
            )
            .with_target(false)
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(log_file))
            .init();
    } else {
        // In stdout mode: write logs to stderr as before
        tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
            )
            .with_target(false)
            .with_writer(std::io::stderr)
            .init();
    }

    if cli.resources.is_empty() && cli.selector.is_none() {
        if use_tui {
            eprintln!("Error: Must specify at least one resource or a label selector (--selector)");
        } else {
            error!("Must specify at least one resource or a label selector (--selector)");
        }
        std::process::exit(1);
    }

    // Initialize clients for all contexts
    let clients = initialize_clients(cli.contexts.clone()).await?;
    let namespace = cli.namespace.as_deref().unwrap_or("default").to_string();

    // Parse resources and selectors (common for both modes)
    let (label_selector_strings, explicit_pods) =
        parse_resources_and_selectors(&clients, &cli, &namespace).await?;

    // Channel for log messages
    let (log_tx, log_rx) = mpsc::channel::<LogMessage>(cli.buffer_size);

    // Branch between TUI and stdout mode
    if use_tui {
        run_tui_mode(
            clients,
            namespace,
            cli,
            label_selector_strings,
            explicit_pods,
            log_tx,
            log_rx,
        )
        .await
    } else {
        run_stdout_mode(
            clients,
            namespace,
            cli,
            label_selector_strings,
            explicit_pods,
            log_tx,
            log_rx,
        )
        .await
    }
}

async fn initialize_clients(contexts: Vec<String>) -> anyhow::Result<Vec<(String, Client)>> {
    let mut clients = Vec::new();

    if contexts.is_empty() {
        // Use current context
        let config = config::Config::infer().await?;
        let client = Client::try_from(config.clone())?;
        // Use cluster URL hostname as context name since current_context is not available in kube::Config
        let context_name = config.cluster_url.host().unwrap_or("default");
        info!("Using current context: {}", context_name);
        clients.push((context_name.to_string(), client));
    } else {
        // Use specified contexts
        for ctx in contexts {
            let config = config::Config::from_kubeconfig(&config::KubeConfigOptions {
                context: Some(ctx.clone()),
                ..Default::default()
            })
            .await?;
            let client = Client::try_from(config)?;
            info!("Initialized client for context: {}", ctx);
            clients.push((ctx, client));
        }
    }

    Ok(clients)
}

async fn parse_resources_and_selectors(
    clients: &[(String, Client)],
    cli: &Cli,
    namespace: &str,
) -> anyhow::Result<(Vec<String>, std::collections::HashSet<String>)> {
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

    // Use first client for resource discovery
    let (_, client) = &clients[0];

    let mut label_selector_strings = Vec::<String>::new();
    let mut explicit_pods = std::collections::HashSet::new();

    for (typ, nam) in &resource_specs {
        if let Some(name) = nam {
            if let Some(sel) = get_selector_from_resource(client, typ, name, namespace).await? {
                if let Some(sel_str) = selector_to_labels_string(&sel) {
                    label_selector_strings.push(sel_str);
                } else {
                    debug!(
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

    Ok((label_selector_strings, explicit_pods))
}

async fn run_stdout_mode(
    clients: Vec<(String, Client)>,
    namespace: String,
    cli: Cli,
    label_selector_strings: Vec<String>,
    explicit_pods: std::collections::HashSet<String>,
    log_tx: mpsc::Sender<LogMessage>,
    mut log_rx: mpsc::Receiver<LogMessage>,
) -> anyhow::Result<()> {
    // Compile grep regex if provided
    let grep_regex = if let Some(pattern) = &cli.grep {
        match Regex::new(pattern) {
            Ok(re) => Some(Arc::new(re)),
            Err(e) => {
                error!("Invalid regex pattern '{}': {}", pattern, e);
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    // Spawn task to print logs
    tokio::spawn(async move {
        while let Some(msg) = log_rx.recv().await {
            if let Some(ref regex) = grep_regex
                && !regex.is_match(&msg.line)
            {
                continue;
            }

            let color_key = format!("{}/{}", msg.cluster, msg.pod_name);
            let color = get_crossterm_color(&color_key);
            let prefix = format!(
                "[{}.{}/{}/{}]",
                msg.cluster, msg.namespace, msg.pod_name, msg.container_name
            )
            .with(color);
            println!("{} {}", prefix, msg.line);
        }
    });

    let handles: Arc<Mutex<HashMap<PodKey, Vec<AbortHandle>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    spawn_all_watchers(
        clients,
        namespace,
        cli,
        label_selector_strings,
        explicit_pods,
        log_tx,
        handles,
        None,
    )
    .await;

    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn run_tui_mode(
    clients: Vec<(String, Client)>,
    namespace: String,
    cli: Cli,
    label_selector_strings: Vec<String>,
    explicit_pods: std::collections::HashSet<String>,
    log_tx: mpsc::Sender<LogMessage>,
    log_rx: mpsc::Receiver<LogMessage>,
) -> anyhow::Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = App::new(cli.buffer_size);

    // Create event channel
    let (event_tx, mut event_rx) = mpsc::channel::<AppEvent>(100);

    // Spawn keyboard event loop
    let event_tx_clone = event_tx.clone();
    tokio::spawn(async move {
        ui::events::event_loop(event_tx_clone).await;
    });

    // Convert log messages to app events
    let event_tx_clone = event_tx.clone();
    tokio::spawn(async move {
        let mut log_rx = log_rx;
        while let Some(msg) = log_rx.recv().await {
            if event_tx_clone
                .send(AppEvent::LogMessage(msg))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let handles: Arc<Mutex<HashMap<PodKey, Vec<AbortHandle>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    spawn_all_watchers(
        clients,
        namespace,
        cli,
        label_selector_strings,
        explicit_pods,
        log_tx,
        handles,
        Some(event_tx.clone()),
    )
    .await;

    // Main TUI event loop
    let mut should_quit = false;
    while !should_quit {
        ui::renderer::render(&mut terminal, &mut app)?;

        if let Some(event) = event_rx.recv().await {
            match event {
                AppEvent::Key(key) => {
                    should_quit = !ui::events::handle_key_event(&mut app, key);
                }
                AppEvent::LogMessage(msg) => {
                    app.add_log(msg);
                }
                AppEvent::PodUpdate(update) => match update.event_type {
                    ui::events::PodEventType::Added | ui::events::PodEventType::Updated => {
                        app.add_pod(update.info);
                    }
                    ui::events::PodEventType::Deleted(key) => {
                        app.remove_pod(&key);
                    }
                },
                AppEvent::Tick => {
                    app.update_stats();
                }
                AppEvent::Quit => {
                    should_quit = true;
                }
            }
        }
    }

    // Cleanup terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn spawn_all_watchers(
    clients: Vec<(String, Client)>,
    namespace: String,
    cli: Cli,
    label_selector_strings: Vec<String>,
    explicit_pods: std::collections::HashSet<String>,
    log_tx: mpsc::Sender<LogMessage>,
    handles: Arc<Mutex<HashMap<PodKey, Vec<AbortHandle>>>>,
    event_tx: Option<mpsc::Sender<AppEvent>>,
) {
    for (cluster_name, client) in clients {
        let ctx = TailContext {
            client: client.clone(),
            cluster: cluster_name.clone(),
            namespace: namespace.clone(),
            container: cli.container.clone(),
            tx: log_tx.clone(),
            tail: cli.tail,
        };

        // Spawn watchers for label selectors
        for selector in &label_selector_strings {
            let pods_api: Api<Pod> = Api::namespaced(ctx.client.clone(), &ctx.namespace);
            let handles = handles.clone();
            let ctx = ctx.clone();
            let selector = selector.clone();
            let event_tx = event_tx.clone();

            tokio::spawn(async move {
                let cfg = WatcherConfig::default().labels(&selector);
                let cluster_name = ctx.cluster.clone();
                debug!(
                    "[{}] Starting watcher for selector: {}",
                    cluster_name, selector
                );
                if let Err(err) = watch_pods(pods_api, cfg, handles, ctx, event_tx).await {
                    warn!(
                        "[{}] Watcher with selector {} stopped: {}",
                        cluster_name, selector, err
                    );
                }
            });
        }

        // Spawn watchers for explicit pods
        for pod_name in &explicit_pods {
            let pods_api: Api<Pod> = Api::namespaced(ctx.client.clone(), &ctx.namespace);
            let handles = handles.clone();
            let ctx = ctx.clone();
            let pod_name = pod_name.clone();
            let event_tx = event_tx.clone();

            tokio::spawn(async move {
                let field_selector = format!("metadata.name={}", pod_name);
                let cfg = WatcherConfig::default().fields(&field_selector);
                let cluster_name = ctx.cluster.clone();
                debug!("[{}] Starting watcher for pod: {}", cluster_name, pod_name);
                if let Err(err) = watch_pods(pods_api, cfg, handles, ctx, event_tx).await {
                    warn!(
                        "[{}] Watcher for pod {} stopped: {}",
                        cluster_name, pod_name, err
                    );
                }
            });
        }
    }
}

async fn watch_pods(
    pods_api: Api<Pod>,
    cfg: WatcherConfig,
    handles: Arc<Mutex<HashMap<PodKey, Vec<AbortHandle>>>>,
    ctx: TailContext,
    event_tx: Option<mpsc::Sender<AppEvent>>,
) -> anyhow::Result<()> {
    let mut stream = watcher(pods_api, cfg).boxed();
    while let Some(event) = stream.try_next().await? {
        match event {
            Event::Apply(pod) | Event::InitApply(pod) => {
                handle_pod_event(pod, &handles, ctx.clone(), event_tx.clone()).await;
            }
            Event::Delete(pod) => {
                let name = pod.name_any();
                let key = PodKey {
                    cluster: ctx.cluster.clone(),
                    namespace: ctx.namespace.clone(),
                    pod_name: name.clone(),
                    container_name: String::new(), // Will match all containers
                };

                info!("[{}] POD DELETED: {}", ctx.cluster, name);
                stop_tailing_pod(&key, &handles).await;

                if let Some(ref tx) = event_tx {
                    let _ = tx
                        .send(AppEvent::PodUpdate(ui::events::PodUpdateEvent {
                            info: PodInfo {
                                key: key.clone(),
                                phase: "Deleted".to_string(),
                                age: String::new(),
                                restarts: 0,
                            },
                            event_type: ui::events::PodEventType::Deleted(key),
                        }))
                        .await;
                }
            }
            Event::Init => {
                info!(
                    "[{}] Initializing pod watcher for namespace: {}",
                    ctx.cluster, ctx.namespace
                );
            }
            Event::InitDone => {
                info!(
                    "[{}] Pod watcher initialization complete for namespace: {}",
                    ctx.cluster, ctx.namespace
                );
            }
        }
    }
    Ok(())
}

async fn handle_pod_event(
    pod: Pod,
    handles: &Arc<Mutex<HashMap<PodKey, Vec<AbortHandle>>>>,
    ctx: TailContext,
    event_tx: Option<mpsc::Sender<AppEvent>>,
) {
    let name = pod.name_any();
    let phase = pod
        .status
        .as_ref()
        .and_then(|s| s.phase.as_ref())
        .unwrap_or(&"Unknown".to_string())
        .clone();

    let is_running = phase == "Running";
    let _is_terminating = pod.metadata.deletion_timestamp.is_some();

    // Create a base key for this pod (container_name will be added per container)
    let base_key = PodKey {
        cluster: ctx.cluster.clone(),
        namespace: ctx.namespace.clone(),
        pod_name: name.clone(),
        container_name: String::new(), // Temporary, will be set per container
    };

    // Check if tracking any containers for this pod
    let guard = handles.lock().await;
    let was_tracking = guard.keys().any(|k| {
        k.cluster == base_key.cluster
            && k.namespace == base_key.namespace
            && k.pod_name == base_key.pod_name
    });
    drop(guard);

    if !was_tracking && (is_running || phase == "Pending") {
        // New pod - start tailing
        info!("[{}] NEW POD: {} | Phase: {}", ctx.cluster, name, phase);

        let pod_handles = spawn_tail_tasks_for_pod(
            ctx.client.clone(),
            ctx.cluster.clone(),
            name.clone(),
            ctx.namespace.clone(),
            ctx.container.clone(),
            ctx.tx.clone(),
            ctx.tail,
        )
        .await;

        // Get container names and create pod info
        if let Some(spec) = &pod.spec {
            for (handle, container) in pod_handles.iter().zip(&spec.containers) {
                let key = PodKey {
                    cluster: ctx.cluster.clone(),
                    namespace: ctx.namespace.clone(),
                    pod_name: name.clone(),
                    container_name: container.name.clone(),
                };

                handles
                    .lock()
                    .await
                    .insert(key.clone(), vec![handle.clone()]);

                // Send pod info to TUI
                if let Some(ref tx) = event_tx {
                    let info = PodInfo {
                        key,
                        phase: phase.clone(),
                        age: format_age(&pod),
                        restarts: get_restart_count(&pod),
                    };
                    let _ = tx
                        .send(AppEvent::PodUpdate(ui::events::PodUpdateEvent {
                            info,
                            event_type: ui::events::PodEventType::Added,
                        }))
                        .await;
                }
            }
        }

        debug!("[{}] Started tailing pod {}", ctx.cluster, name);
    } else if was_tracking && !is_running {
        info!(
            "[{}] POD STATUS CHANGED: {} | Phase: {} | Stopped tailing",
            ctx.cluster, name, phase
        );
        stop_tailing_pod(&base_key, handles).await;
    }
}

async fn stop_tailing_pod(
    base_key: &PodKey,
    handles: &Arc<Mutex<HashMap<PodKey, Vec<AbortHandle>>>>,
) {
    let mut guard = handles.lock().await;

    // Find and remove all handles for this pod (all containers)
    let keys_to_remove: Vec<PodKey> = guard
        .keys()
        .filter(|k| {
            k.cluster == base_key.cluster
                && k.namespace == base_key.namespace
                && k.pod_name == base_key.pod_name
        })
        .cloned()
        .collect();

    for key in keys_to_remove {
        if let Some(pod_handles) = guard.remove(&key) {
            for handle in pod_handles {
                handle.abort();
            }
        }
    }
}

fn format_age(pod: &Pod) -> String {
    pod.metadata
        .creation_timestamp
        .as_ref()
        .map(|t| {
            let now = chrono::Utc::now();
            let created = t.0;
            let duration = now.signed_duration_since(created);
            let total_secs = duration.num_seconds();
            if total_secs < 60 {
                format!("{}s", total_secs)
            } else if total_secs < 3600 {
                format!("{}m", total_secs / 60)
            } else if total_secs < 86400 {
                format!("{}h", total_secs / 3600)
            } else {
                format!("{}d", total_secs / 86400)
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn get_restart_count(pod: &Pod) -> i32 {
    pod.status
        .as_ref()
        .and_then(|s| s.container_statuses.as_ref())
        .map(|statuses| statuses.iter().map(|cs| cs.restart_count).sum())
        .unwrap_or(0)
}
