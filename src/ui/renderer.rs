use crate::ui::app::{App, PodInfo};
use crate::ui::layout::create_layout;
use crate::ui::widgets::{HelpOverlay, LogView, PodList, StatusBar};
use ratatui::{Frame, Terminal, backend::Backend};

pub fn render<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> std::io::Result<()> {
    terminal.draw(|f| render_frame(f, app))?;
    Ok(())
}

fn render_frame(f: &mut Frame, app: &mut App) {
    let layout = create_layout(f.area(), app.sidebar_visible);

    // Render sidebar if visible
    if app.sidebar_visible {
        // Build mapping from list index to container key
        use std::collections::BTreeMap;
        let mut tree: BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<&PodInfo>>>> =
            BTreeMap::new();

        for pod in &app.pods {
            tree.entry(pod.key.cluster.clone())
                .or_default()
                .entry(pod.key.namespace.clone())
                .or_default()
                .entry(pod.key.pod_name.clone())
                .or_default()
                .push(pod);
        }

        // Build the item keys and types mapping
        app.sidebar_item_keys.clear();
        app.sidebar_item_types.clear();
        for (cluster, namespaces) in &tree {
            app.sidebar_item_keys.push(None); // Cluster header
            app.sidebar_item_types
                .push(crate::ui::app::TreeNodeType::Cluster(cluster.clone()));

            // Only show children if cluster is expanded
            if app.expanded_nodes.contains(cluster) {
                for (namespace, pods) in namespaces {
                    app.sidebar_item_keys.push(None); // Namespace header
                    app.sidebar_item_types.push(crate::ui::app::TreeNodeType::Namespace(
                        cluster.clone(),
                        namespace.clone(),
                    ));

                    // Only show children if namespace is expanded
                    let ns_path = format!("{}/{}", cluster, namespace);
                    if app.expanded_nodes.contains(&ns_path) {
                        for (pod_name, containers) in pods {
                            app.sidebar_item_keys.push(None); // Pod header
                            app.sidebar_item_types.push(crate::ui::app::TreeNodeType::Pod(
                                cluster.clone(),
                                namespace.clone(),
                                pod_name.clone(),
                            ));

                            // Only show children if pod is expanded
                            let pod_path = format!("{}/{}/{}", cluster, namespace, pod_name);
                            if app.expanded_nodes.contains(&pod_path) {
                                for container in containers {
                                    app.sidebar_item_keys.push(Some(container.key.clone())); // Container (selectable)
                                    app.sidebar_item_types
                                        .push(crate::ui::app::TreeNodeType::Container);
                                }
                            }
                        }
                    }
                }
            }
        }

        let pod_list = PodList::new(&app.pods, &app.pod_states, &app.expanded_nodes);
        f.render_stateful_widget(pod_list, layout.sidebar, &mut app.sidebar_state);
    }

    // Render log view
    let filtered_logs = app.filtered_logs();
    let log_view = LogView::new(
        filtered_logs,
        app.scroll_offset,
        &app.search_pattern,
        app.show_timestamps,
        app.show_prefix,
    );
    f.render_widget(log_view, layout.main);

    // Render status bar
    let clusters = app.get_clusters();
    let status_bar = StatusBar::new(
        app.running_pods,
        app.total_pods,
        app.log_buffer.len(),
        app.memory_usage,
        &app.active_filters,
        &clusters,
        app.paused,
        app.auto_scroll,
    );
    f.render_widget(status_bar, layout.status_bar);

    // Render help overlay if visible
    if app.help_visible {
        f.render_widget(HelpOverlay, f.area());
    }

    // Render search bar if in search mode
    if app.mode == crate::ui::app::AppMode::Search {
        use ratatui::{
            layout::{Alignment, Constraint, Direction, Layout},
            style::{Color, Style},
            text::Span,
            widgets::{Block, Borders, Clear, Paragraph},
        };

        let search_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(f.area())[1];

        // Clear the area to make it opaque
        f.render_widget(Clear, search_area);

        let search_text = format!("Search: {}_", app.search_pattern);
        let search_widget = Paragraph::new(Span::styled(
            search_text,
            Style::default().fg(Color::Yellow),
        ))
        .block(
            Block::default()
                .title("Search (Enter to apply, Esc to cancel)")
                .borders(Borders::ALL)
                .style(Style::default().fg(Color::Yellow)),
        )
        .alignment(Alignment::Left);

        f.render_widget(search_widget, search_area);
    }

    // Render filter bar if in filter mode
    if app.mode == crate::ui::app::AppMode::Filter {
        use ratatui::{
            layout::{Alignment, Constraint, Direction, Layout},
            style::{Color, Style},
            text::Span,
            widgets::{Block, Borders, Clear, Paragraph},
        };

        let filter_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(f.area())[1];

        // Clear the area to make it opaque
        f.render_widget(Clear, filter_area);

        let filter_text = format!("Filter: {}_", app.filter_pattern);
        let filter_widget =
            Paragraph::new(Span::styled(filter_text, Style::default().fg(Color::Cyan)))
                .block(
                    Block::default()
                        .title("Filter (Enter to apply, Esc to cancel)")
                        .borders(Borders::ALL)
                        .style(Style::default().fg(Color::Cyan)),
                )
                .alignment(Alignment::Left);

        f.render_widget(filter_widget, filter_area);
    }
}
