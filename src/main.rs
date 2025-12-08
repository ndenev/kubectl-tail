mod cli;
mod kubernetes;
mod types;

use clap::Parser;
use crossterm::style::{Color, Stylize};
use futures::stream::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::{
    Api, Client, ResourceExt,
    api::{ListParams, WatchParams},
    config,
};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

use cli::Cli;
use kubernetes::{get_selector_from_resource, spawn_tail_tasks_for_pod};
use types::LogMessage;

fn parse_labels(sel_str: &str) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for pair in sel_str.split(',') {
        let pair = pair.trim();
        if let Some(eq_pos) = pair.find('=') {
            let key = pair[..eq_pos].to_string();
            let value = pair[eq_pos + 1..].to_string();
            map.insert(key, value);
        }
    }
    map
}

fn selector_to_labels_string(selector: &LabelSelector) -> Option<String> {
    if let Some(labels) = &selector.match_labels {
        if labels.is_empty() {
            None
        } else {
            Some(
                labels
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(","),
            )
        }
    } else {
        None
    }
}

fn matches_selector(
    pod_labels: &std::collections::BTreeMap<String, String>,
    selector: &LabelSelector,
) -> bool {
    // Check match_labels
    if let Some(match_labels) = &selector.match_labels {
        for (key, value) in match_labels {
            if pod_labels.get(key) != Some(value) {
                return false;
            }
        }
    }
    // Check match_expressions
    if let Some(expressions) = &selector.match_expressions {
        for expr in expressions {
            let pod_value = pod_labels.get(&expr.key);
            match expr.operator.as_str() {
                "In" => {
                    if let Some(values) = &expr.values {
                        if pod_value.is_none() || !values.contains(pod_value.unwrap()) {
                            return false;
                        }
                    } else {
                        return false; // No values for In
                    }
                }
                "NotIn" => {
                    if let Some(values) = &expr.values {
                        if pod_value.is_some() && values.contains(pod_value.unwrap()) {
                            return false;
                        }
                    } else {
                        return false; // No values for NotIn
                    }
                }
                "Exists" => {
                    if pod_value.is_none() {
                        return false;
                    }
                }
                "DoesNotExist" => {
                    if pod_value.is_some() {
                        return false;
                    }
                }
                _ => return false, // Unknown operator
            }
        }
    }
    true
}

fn get_color(s: &str) -> Color {
    let colors = [
        Color::Red,
        Color::Green,
        Color::Blue,
        Color::Yellow,
        Color::Magenta,
        Color::Cyan,
        Color::White,
        Color::Grey,
        Color::AnsiValue(91), // Bright Red
        Color::AnsiValue(92), // Bright Green
        Color::AnsiValue(94), // Bright Blue
        Color::AnsiValue(93), // Bright Yellow
        Color::AnsiValue(95), // Bright Magenta
        Color::AnsiValue(96), // Bright Cyan
    ];
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    let hash = hasher.finish() as u32;
    colors[(hash % colors.len() as u32) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_parsing_deployment() {
        let args = vec!["kubectl-tail", "deployment/my-deployment"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.resources, vec!["deployment/my-deployment".to_string()]);
        assert!(cli.selector.is_none());
    }

    #[test]
    fn test_cli_parsing_pod() {
        let args = vec!["kubectl-tail", "my-pod"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.resources, vec!["my-pod".to_string()]);
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

    #[test]
    fn test_cli_parsing_multiple_resources() {
        let args = vec!["kubectl-tail", "deployment/app1", "pod/my-pod"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(
            cli.resources,
            vec!["deployment/app1".to_string(), "pod/my-pod".to_string()]
        );
    }

    #[test]
    fn test_cli_parsing_tail() {
        let args = vec!["kubectl-tail", "pod/my-pod", "--tail", "10"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.tail, Some(10));
    }

    #[test]
    fn test_cli_parsing_verbose() {
        let args = vec!["kubectl-tail", "pod/my-pod", "-v"];
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(cli.verbose);
    }

    #[test]
    fn test_parse_labels() {
        let result = parse_labels("app=nginx,version=v1");
        let mut expected = std::collections::BTreeMap::new();
        expected.insert("app".to_string(), "nginx".to_string());
        expected.insert("version".to_string(), "v1".to_string());
        assert_eq!(result, expected);
    }

    #[test]
    fn test_selector_to_labels_string() {
        let mut labels = std::collections::BTreeMap::new();
        labels.insert("app".to_string(), "nginx".to_string());
        labels.insert("version".to_string(), "v1".to_string());
        let selector = LabelSelector {
            match_labels: Some(labels),
            match_expressions: None,
        };
        let result = selector_to_labels_string(&selector);
        assert_eq!(result, Some("app=nginx,version=v1".to_string()));
    }

    #[test]
    fn test_matches_selector_labels() {
        let mut pod_labels = std::collections::BTreeMap::new();
        pod_labels.insert("app".to_string(), "nginx".to_string());
        pod_labels.insert("version".to_string(), "v1".to_string());

        let mut sel_labels = std::collections::BTreeMap::new();
        sel_labels.insert("app".to_string(), "nginx".to_string());
        let selector = LabelSelector {
            match_labels: Some(sel_labels),
            match_expressions: None,
        };

        assert!(matches_selector(&pod_labels, &selector));
    }

    #[test]
    fn test_matches_selector_expressions_in() {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelectorRequirement;

        let mut pod_labels = std::collections::BTreeMap::new();
        pod_labels.insert("env".to_string(), "prod".to_string());

        let expr = LabelSelectorRequirement {
            key: "env".to_string(),
            operator: "In".to_string(),
            values: Some(vec!["prod".to_string(), "dev".to_string()]),
        };
        let selector = LabelSelector {
            match_labels: None,
            match_expressions: Some(vec![expr]),
        };

        assert!(matches_selector(&pod_labels, &selector));
    }

    #[test]
    fn test_matches_selector_expressions_exists() {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelectorRequirement;

        let mut pod_labels = std::collections::BTreeMap::new();
        pod_labels.insert("app".to_string(), "nginx".to_string());

        let expr = LabelSelectorRequirement {
            key: "app".to_string(),
            operator: "Exists".to_string(),
            values: None,
        };
        let selector = LabelSelector {
            match_labels: None,
            match_expressions: Some(vec![expr]),
        };

        assert!(matches_selector(&pod_labels, &selector));
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

    fn parse_labels(sel_str: &str) -> std::collections::BTreeMap<String, String> {
        let mut map = std::collections::BTreeMap::new();
        for pair in sel_str.split(',') {
            let pair = pair.trim();
            if let Some(eq_pos) = pair.find('=') {
                let key = pair[..eq_pos].to_string();
                let value = pair[eq_pos + 1..].to_string();
                map.insert(key, value);
            }
        }
        map
    }

    fn selector_to_labels_string(selector: &LabelSelector) -> Option<String> {
        if let Some(labels) = &selector.match_labels {
            if labels.is_empty() {
                None
            } else {
                Some(
                    labels
                        .iter()
                        .map(|(k, v)| format!("{}={}", k, v))
                        .collect::<Vec<_>>()
                        .join(","),
                )
            }
        } else {
            None
        }
    }

    fn matches_selector(
        pod_labels: &std::collections::BTreeMap<String, String>,
        selector: &LabelSelector,
    ) -> bool {
        // Check match_labels
        if let Some(match_labels) = &selector.match_labels {
            for (key, value) in match_labels {
                if pod_labels.get(key) != Some(value) {
                    return false;
                }
            }
        }
        // Check match_expressions
        if let Some(expressions) = &selector.match_expressions {
            for expr in expressions {
                let pod_value = pod_labels.get(&expr.key);
                match expr.operator.as_str() {
                    "In" => {
                        if let Some(values) = &expr.values {
                            if pod_value.is_none() || !values.contains(pod_value.unwrap()) {
                                return false;
                            }
                        } else {
                            return false; // No values for In
                        }
                    }
                    "NotIn" => {
                        if let Some(values) = &expr.values {
                            if pod_value.is_some() && values.contains(pod_value.unwrap()) {
                                return false;
                            }
                        } else {
                            return false; // No values for NotIn
                        }
                    }
                    "Exists" => {
                        if pod_value.is_none() {
                            return false;
                        }
                    }
                    "DoesNotExist" => {
                        if pod_value.is_some() {
                            return false;
                        }
                    }
                    _ => return false, // Unknown operator
                }
            }
        }
        true
    }

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
                                        && labels.map_or(false, |l| {
                                            all_selectors_clone
                                                .iter()
                                                .any(|sel| matches_selector(l, sel))
                                        })
                                    {
                                        pod_names.push(name.clone());
                                        println!("New pod added: {}", name);
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
                                println!("Pod deleted: {}", name);
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
                                        && labels.map_or(false, |l| {
                                            all_selectors_clone
                                                .iter()
                                                .any(|sel| matches_selector(l, sel))
                                        })
                                    {
                                        pod_names.push(name.clone());
                                        println!("Pod became running: {}", name);
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
                                    println!("Pod stopped running: {}", name);
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
