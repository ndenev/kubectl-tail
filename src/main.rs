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

/// Configuration for watching pods in a specific context/namespace
#[derive(Debug, Clone)]
struct WatchConfig {
    context: String,
    namespace: String,
    label_selectors: Vec<String>,
    explicit_pods: std::collections::HashSet<String>,
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

    // Extract contexts from resource specs
    let mut contexts_to_init = std::collections::HashSet::new();

    // Add explicit --context flag if present
    if let Some(ctx) = &cli.context {
        contexts_to_init.insert(ctx.clone());
    }

    // Parse resource specs to extract contexts
    for res in &cli.resources {
        if let Ok(spec) = parse_resource_spec(res)
            && let Some(ctx) = spec.context
        {
            contexts_to_init.insert(ctx);
        }
    }

    // Initialize clients for all contexts
    let clients = initialize_clients(contexts_to_init.into_iter().collect()).await?;

    // Parse resources and selectors (common for both modes)
    let watch_configs = parse_resources_and_selectors(&clients, &cli).await?;

    // Channel for log messages
    let (log_tx, log_rx) = mpsc::channel::<LogMessage>(cli.buffer_size);

    // Branch between TUI and stdout mode
    if use_tui {
        run_tui_mode(clients, cli, watch_configs, log_tx, log_rx).await
    } else {
        run_stdout_mode(clients, cli, watch_configs, log_tx, log_rx).await
    }
}

async fn initialize_clients(context_names: Vec<String>) -> anyhow::Result<Vec<(String, Client)>> {
    let mut clients = Vec::new();

    if context_names.is_empty() {
        // Use current context - read kubeconfig to get the actual context name
        let kubeconfig = config::Kubeconfig::read()?;
        let current_context_name = kubeconfig
            .current_context
            .as_deref()
            .unwrap_or("default")
            .to_string();

        let config = config::Config::infer().await?;
        let client = Client::try_from(config)?;
        info!("Using current context: {}", current_context_name);
        clients.push((current_context_name, client));
    } else {
        // Use specified contexts - validate they exist
        for ctx in context_names {
            let config = config::Config::from_kubeconfig(&config::KubeConfigOptions {
                context: Some(ctx.clone()),
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow::anyhow!("Context '{}' not found in kubeconfig: {}", ctx, e))?;
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
) -> anyhow::Result<Vec<WatchConfig>> {
    use std::collections::{HashMap, HashSet};
    use types::ResourceSpec;

    // Parse all resource specs
    let mut parsed_specs = Vec::new();
    for res in &cli.resources {
        let spec = parse_resource_spec(res)
            .map_err(|e| anyhow::anyhow!("Failed to parse resource '{}': {}", res, e))?;
        parsed_specs.push(spec);
    }

    // Validate: if --context or --namespace flags are used, resource specs can't override them
    if cli.context.is_some() {
        for spec in &parsed_specs {
            if spec.context.is_some() {
                anyhow::bail!(
                    "Cannot use both --context flag and context in resource spec '{}/{}/{}/{}'",
                    spec.context.as_ref().unwrap(),
                    spec.namespace.as_deref().unwrap_or("?"),
                    spec.kind.as_deref().unwrap_or("pod"),
                    spec.name
                );
            }
        }
    }

    if cli.namespace.is_some() {
        for spec in &parsed_specs {
            if spec.namespace.is_some() {
                anyhow::bail!(
                    "Cannot use both --namespace flag and namespace in resource spec '{}/{}'",
                    spec.namespace.as_ref().unwrap(),
                    spec.name
                );
            }
        }
    }

    // Determine default context and namespace
    let default_context = if let Some(ctx) = &cli.context {
        ctx.clone()
    } else if !clients.is_empty() {
        clients[0].0.clone()
    } else {
        anyhow::bail!("No context available");
    };

    let default_namespace = cli.namespace.as_deref().unwrap_or("default");

    // Group resources by (context, namespace)
    let mut grouped: HashMap<(String, String), Vec<ResourceSpec>> = HashMap::new();

    for spec in parsed_specs {
        let ctx = spec
            .context
            .as_deref()
            .unwrap_or(&default_context)
            .to_string();
        let ns = spec
            .namespace
            .as_deref()
            .unwrap_or(default_namespace)
            .to_string();
        grouped.entry((ctx, ns)).or_default().push(spec);
    }

    // Add label selector as a separate entry if provided
    if cli.selector.is_some() {
        grouped
            .entry((default_context.clone(), default_namespace.to_string()))
            .or_default();
    }

    // Build WatchConfig for each (context, namespace) group
    let mut configs = Vec::new();

    for ((ctx, ns), specs) in grouped {
        // Find the client for this context
        let client = clients
            .iter()
            .find(|(c, _)| c == &ctx)
            .map(|(_, client)| client)
            .ok_or_else(|| anyhow::anyhow!("No client found for context '{}'", ctx))?;

        let mut label_selectors = Vec::new();
        let mut explicit_pods = HashSet::new();

        // Process each resource spec in this group
        for spec in specs {
            let kind = spec.kind.as_deref().unwrap_or("pod");
            let name = &spec.name;

            // Try to get selector - be resilient to errors
            match get_selector_from_resource(client, kind, name, &ns).await {
                Ok(Some(sel)) => {
                    if let Some(sel_str) = selector_to_labels_string(&sel) {
                        label_selectors.push(sel_str);
                    } else {
                        debug!(
                            "[{}] Selector for {}/{} is empty; skipping",
                            ctx, kind, name
                        );
                    }
                }
                Ok(None) => {
                    if kind == "pod" {
                        explicit_pods.insert(name.clone());
                    }
                }
                Err(e) => {
                    // Don't fail - just log and continue
                    warn!(
                        "[{}] Could not get selector for {}/{} in namespace {}: {}. Will wait for it to appear.",
                        ctx, kind, name, ns, e
                    );
                }
            }
        }

        // Add label selector from CLI if this is the default context/namespace
        if ctx == default_context
            && ns == default_namespace
            && let Some(sel_str) = &cli.selector
        {
            label_selectors.push(sel_str.clone());
        }

        configs.push(WatchConfig {
            context: ctx,
            namespace: ns,
            label_selectors,
            explicit_pods,
        });
    }

    Ok(configs)
}

async fn run_stdout_mode(
    clients: Vec<(String, Client)>,
    cli: Cli,
    watch_configs: Vec<WatchConfig>,
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

    spawn_all_watchers(clients, cli, watch_configs, log_tx, handles, None).await;

    tokio::signal::ctrl_c().await?;
    Ok(())
}

async fn run_tui_mode(
    clients: Vec<(String, Client)>,
    cli: Cli,
    watch_configs: Vec<WatchConfig>,
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
        cli,
        watch_configs,
        log_tx,
        handles,
        Some(event_tx.clone()),
    )
    .await;

    // Main TUI event loop with render throttling
    let mut should_quit = false;
    let mut render_interval = tokio::time::interval(std::time::Duration::from_millis(16)); // ~60 FPS
    render_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    while !should_quit {
        tokio::select! {
            _ = render_interval.tick() => {
                // Render at fixed interval
                ui::renderer::render(&mut terminal, &mut app)?;
            }
            event = event_rx.recv() => {
                if let Some(event) = event {
                    // Process this event
                    match event {
                        AppEvent::Key(key) => {
                            should_quit = !ui::events::handle_key_event(&mut app, key);
                            // Render immediately after keyboard input for responsiveness
                            ui::renderer::render(&mut terminal, &mut app)?;
                        }
                        AppEvent::LogMessage(msg) => {
                            app.add_log(msg);
                            // Batch process additional log messages without blocking
                            while let Ok(AppEvent::LogMessage(msg)) = event_rx.try_recv() {
                                app.add_log(msg);
                            }
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
    cli: Cli,
    watch_configs: Vec<WatchConfig>,
    log_tx: mpsc::Sender<LogMessage>,
    handles: Arc<Mutex<HashMap<PodKey, Vec<AbortHandle>>>>,
    event_tx: Option<mpsc::Sender<AppEvent>>,
) {
    // Group configs by context for easy client lookup
    let client_map: std::collections::HashMap<_, _> = clients.into_iter().collect();

    for config in watch_configs {
        let client = match client_map.get(&config.context) {
            Some(c) => c.clone(),
            None => {
                warn!("[{}] No client found for context, skipping", config.context);
                continue;
            }
        };

        let ctx = TailContext {
            client: client.clone(),
            cluster: config.context.clone(),
            namespace: config.namespace.clone(),
            container: cli.container.clone(),
            tx: log_tx.clone(),
            tail: cli.tail,
        };

        // Spawn watchers for label selectors
        for selector in &config.label_selectors {
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
        for pod_name in &config.explicit_pods {
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
