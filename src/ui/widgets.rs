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
use std::collections::HashMap;

pub struct PodList<'a> {
    pods: &'a [PodInfo],
    states: &'a HashMap<PodKey, PodState>,
}

impl<'a> PodList<'a> {
    pub fn new(pods: &'a [PodInfo], states: &'a HashMap<PodKey, PodState>) -> Self {
        Self { pods, states }
    }
}

impl<'a> StatefulWidget for PodList<'a> {
    type State = ratatui::widgets::ListState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let items: Vec<ListItem> = self
            .pods
            .iter()
            .map(|pod| {
                let enabled = self.states.get(&pod.key).map(|s| s.enabled).unwrap_or(true);
                let checkbox = if enabled { "[x]" } else { "[ ]" };

                // Format: [x] cluster/pod/container (phase)
                let text = format!(
                    "{} {}/{}/{} ({})",
                    checkbox, pod.key.cluster, pod.key.pod_name, pod.key.container_name, pod.phase
                );

                let style = if enabled {
                    Style::default()
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                ListItem::new(text).style(style)
            })
            .collect();

        let list = List::new(items)
            .block(Block::default().borders(Borders::RIGHT))
            .highlight_style(
                Style::default()
                    .add_modifier(Modifier::REVERSED)
                    .fg(Color::Yellow),
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
            if let Ok(regex) = Regex::new(self.search_pattern) {
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
            "  ↑/↓         - Scroll or navigate sidebar",
            "  PgUp/PgDn   - Page scroll",
            "  Home/End    - Jump to top/bottom",
            "  Space       - Toggle pod/container (in sidebar)",
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
