use crate::types::LogMessage;
use crate::ui::app::{PodInfo, PodKey, PodState};
use crate::utils::get_color;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, StatefulWidget, Widget, Wrap},
};
use regex::Regex;
use std::collections::{HashMap, HashSet};

pub struct PodList<'a> {
    pods: &'a [PodInfo],
    states: &'a HashMap<PodKey, PodState>,
    expanded_nodes: &'a HashSet<String>,
}

impl<'a> PodList<'a> {
    pub fn new(
        pods: &'a [PodInfo],
        states: &'a HashMap<PodKey, PodState>,
        expanded_nodes: &'a HashSet<String>,
    ) -> Self {
        Self {
            pods,
            states,
            expanded_nodes,
        }
    }
}

impl<'a> StatefulWidget for PodList<'a> {
    type State = ratatui::widgets::ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        use std::collections::BTreeMap;

        // Group pods by cluster -> namespace -> pod -> containers
        let mut tree: BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<&PodInfo>>>> =
            BTreeMap::new();

        for pod in self.pods {
            tree.entry(pod.key.cluster.clone())
                .or_default()
                .entry(pod.key.namespace.clone())
                .or_default()
                .entry(pod.key.pod_name.clone())
                .or_default()
                .push(pod);
        }

        // Helper function to calculate selection state for a group of containers
        let calc_selection_state = |containers: &[&PodInfo]| -> (usize, usize) {
            let total = containers.len();
            let enabled = containers
                .iter()
                .filter(|c| {
                    self.states
                        .get(&c.key)
                        .map(|s| s.enabled)
                        .unwrap_or(true)
                })
                .count();
            (enabled, total)
        };

        // Build flat list with tree structure
        let mut items: Vec<ListItem> = Vec::new();

        for (cluster, namespaces) in &tree {
            // Calculate cluster selection state
            let mut cluster_enabled = 0;
            let mut cluster_total = 0;
            for pods in namespaces.values() {
                for containers in pods.values() {
                    let (enabled, total) = calc_selection_state(containers);
                    cluster_enabled += enabled;
                    cluster_total += total;
                }
            }

            // Determine cluster style based on selection state
            let cluster_style = if cluster_total == 0 {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else if cluster_enabled == 0 {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else if cluster_enabled < cluster_total {
                Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            };

            // Cluster header
            let cluster_expanded = self.expanded_nodes.contains(cluster);
            let cluster_icon = if cluster_expanded { "▼" } else { "▶" };
            items.push(
                ListItem::new(format!("{} {}", cluster_icon, cluster)).style(cluster_style),
            );

            // Only show children if cluster is expanded
            if cluster_expanded {
                for (namespace, pods) in namespaces {
                    // Calculate namespace selection state
                    let mut ns_enabled = 0;
                    let mut ns_total = 0;
                    for containers in pods.values() {
                        let (enabled, total) = calc_selection_state(containers);
                        ns_enabled += enabled;
                        ns_total += total;
                    }

                    let ns_style = if ns_total == 0 {
                        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
                    } else if ns_enabled == 0 {
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD)
                    } else if ns_enabled < ns_total {
                        Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)
                    };

                    // Namespace header (indented)
                    let ns_path = format!("{}/{}", cluster, namespace);
                    let ns_expanded = self.expanded_nodes.contains(&ns_path);
                    let ns_icon = if ns_expanded { "▼" } else { "▶" };
                    items.push(
                        ListItem::new(format!("  {} {}", ns_icon, namespace)).style(ns_style),
                    );

                    // Only show children if namespace is expanded
                    if ns_expanded {
                        for (pod_name, containers) in pods {
                            // Calculate pod selection state
                            let (pod_enabled, pod_total) = calc_selection_state(containers);

                            let pod_style = if pod_total == 0 {
                                Style::default().fg(Color::Green)
                            } else if pod_enabled == 0 {
                                Style::default().fg(Color::DarkGray)
                            } else if pod_enabled < pod_total {
                                Style::default().fg(Color::Gray)
                            } else {
                                Style::default().fg(Color::Green)
                            };

                            // Pod header (indented more)
                            let phase = &containers[0].phase;
                            let pod_path = format!("{}/{}/{}", cluster, namespace, pod_name);
                            let pod_expanded = self.expanded_nodes.contains(&pod_path);
                            let pod_icon = if pod_expanded { "▼" } else { "▶" };
                            items.push(
                                ListItem::new(format!(
                                    "    {} {} ({})",
                                    pod_icon, pod_name, phase
                                ))
                                .style(pod_style),
                            );

                            // Only show children if pod is expanded
                            if pod_expanded {
                                // Containers (indented most)
                                for container in containers {
                                    let enabled = self
                                        .states
                                        .get(&container.key)
                                        .map(|s| s.enabled)
                                        .unwrap_or(true);
                                    let checkbox = if enabled { "[x]" } else { "[ ]" };

                                    let text = format!(
                                        "      {} {}",
                                        checkbox, container.key.container_name
                                    );

                                    let style = if enabled {
                                        Style::default()
                                    } else {
                                        Style::default().fg(Color::DarkGray)
                                    };

                                    items.push(ListItem::new(text).style(style));
                                }
                            }
                        }
                    }
                }
            }
        }

        let list = List::new(items)
            .block(Block::default().borders(Borders::RIGHT))
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::REVERSED)
                    .fg(Color::Yellow)
                    .bg(Color::DarkGray),
            )
            .highlight_symbol("→ ");

        StatefulWidget::render(list, area, buf, state);
    }
}

pub struct LogView<'a> {
    logs: Vec<&'a LogMessage>,
    scroll_offset: usize,
    search_pattern: &'a str,
    show_timestamps: bool,
    show_prefix: bool,
}

impl<'a> LogView<'a> {
    pub fn new(
        logs: Vec<&'a LogMessage>,
        scroll_offset: usize,
        search_pattern: &'a str,
        show_timestamps: bool,
        show_prefix: bool,
    ) -> Self {
        Self {
            logs,
            scroll_offset,
            search_pattern,
            show_timestamps,
            show_prefix,
        }
    }

    fn format_log_line<'b>(&self, msg: &'b LogMessage) -> Line<'b> {
        let color_key = format!("{}/{}", msg.cluster, msg.pod_name);
        let color = get_color(&color_key);

        let mut spans = Vec::new();

        // Add timestamp if enabled
        if self.show_timestamps {
            let ts = msg.timestamp.format("%H:%M:%S%.3f").to_string();
            spans.push(Span::styled(
                format!("{} ", ts),
                Style::default().fg(Color::DarkGray),
            ));
        }

        // Add prefix if enabled: [cluster.namespace/pod/container]
        if self.show_prefix {
            let prefix = format!(
                "[{}.{}/{}/{}]",
                msg.cluster, msg.namespace, msg.pod_name, msg.container_name
            );
            spans.push(Span::styled(prefix, Style::default().fg(color)));
            spans.push(Span::raw(" "));
        }

        // Add log line with highlighting if search pattern is active
        if !self.search_pattern.is_empty() {
            // Make search case-insensitive by default (prepend (?i))
            let pattern = format!("(?i){}", self.search_pattern);
            if let Ok(regex) = Regex::new(&pattern) {
                let mut last_end = 0;
                for mat in regex.find_iter(&msg.line) {
                    // Add text before match
                    if mat.start() > last_end {
                        spans.push(Span::raw(&msg.line[last_end..mat.start()]));
                    }
                    // Add highlighted match
                    spans.push(Span::styled(
                        mat.as_str(),
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ));
                    last_end = mat.end();
                }
                // Add remaining text
                if last_end < msg.line.len() {
                    spans.push(Span::raw(&msg.line[last_end..]));
                }
            } else {
                // Invalid regex, just show the line
                spans.push(Span::raw(&msg.line));
            }
        } else {
            spans.push(Span::raw(&msg.line));
        }

        Line::from(spans)
    }
}

impl<'a> Widget for LogView<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let lines: Vec<Line> = self
            .logs
            .iter()
            .map(|msg| self.format_log_line(msg))
            .collect();

        let paragraph = Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll_offset as u16, 0));

        paragraph.render(area, buf);
    }
}

pub struct StatusBar<'a> {
    running_pods: usize,
    total_pods: usize,
    buffer_lines: usize,
    memory_mb: usize,
    active_filters: &'a [String],
    clusters: &'a [String],
    paused: bool,
    auto_scroll: bool,
}

impl<'a> StatusBar<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        running_pods: usize,
        total_pods: usize,
        buffer_lines: usize,
        memory_mb: usize,
        active_filters: &'a [String],
        clusters: &'a [String],
        paused: bool,
        auto_scroll: bool,
    ) -> Self {
        Self {
            running_pods,
            total_pods,
            buffer_lines,
            memory_mb,
            active_filters,
            clusters,
            paused,
            auto_scroll,
        }
    }
}

impl<'a> Widget for StatusBar<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let clusters_str = if self.clusters.is_empty() {
            "none".to_string()
        } else {
            self.clusters.join(",")
        };

        let filters_str = if self.active_filters.is_empty() {
            "none".to_string()
        } else {
            self.active_filters.join(", ")
        };

        let status_parts = [
            format!("Pods: {}/{}", self.running_pods, self.total_pods),
            format!("Buffer: {} lines", self.buffer_lines),
            format!("Memory: {} MB", self.memory_mb),
            format!("Filters: {}", filters_str),
            format!("Clusters: {}", clusters_str),
        ];

        let mut status_text = status_parts.join(" | ");

        // Add mode indicators
        if self.paused {
            status_text.push_str(" | [PAUSED]");
        }
        if self.auto_scroll {
            status_text.push_str(" | [AUTO]");
        }

        // Add help hint
        status_text.push_str(" | ? for help");

        let paragraph = Paragraph::new(status_text)
            .style(Style::default().bg(Color::DarkGray).fg(Color::White));

        paragraph.render(area, buf);
    }
}

pub struct HelpOverlay;

impl Widget for HelpOverlay {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let help_lines = vec![
            "Keyboard Shortcuts",
            "",
            "  q/Q/Ctrl-C  - Quit",
            "  s           - Toggle sidebar",
            "  p           - Pause/Resume",
            "  c           - Clear log buffer",
            "  a           - Toggle auto-scroll",
            "  t           - Toggle timestamps",
            "  x           - Toggle pod/container prefix",
            "  ?           - Toggle this help",
            "",
            "Search & Filter:",
            "  /           - Search (highlights matches, use n/N to navigate)",
            "  n/N         - Jump to next/previous search match",
            "  f           - Filter (show only matching lines)",
            "",
            "Navigation:",
            "  ↑/↓         - Navigate sidebar (when open) or scroll logs",
            "  PgUp/PgDn   - Page scroll (logs)",
            "  Home/End    - Jump to top/bottom (logs)",
            "  g/G         - Jump to top/bottom (logs, vim-style)",
            "  Space       - Toggle pod/container or expand/collapse tree node",
            "",
            "Press any key to close",
        ];

        let lines: Vec<Line> = help_lines.iter().map(|s| Line::from(*s)).collect();

        // Center the help overlay
        let help_width = 60;
        let help_height = help_lines.len() as u16 + 2;
        let x = (area.width.saturating_sub(help_width)) / 2;
        let y = (area.height.saturating_sub(help_height)) / 2;

        let help_area = Rect {
            x: area.x + x,
            y: area.y + y,
            width: help_width.min(area.width),
            height: help_height.min(area.height),
        };

        // Clear the area to make it opaque
        Clear.render(help_area, buf);

        let block = Block::default()
            .title("Help")
            .borders(Borders::ALL)
            .style(Style::default().bg(Color::Black).fg(Color::White));

        let paragraph = Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Left)
            .style(Style::default().bg(Color::Black).fg(Color::White));

        paragraph.render(help_area, buf);
    }
}
