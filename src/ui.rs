use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Row, Table, Cell},
    Frame, Terminal,
};
use std::{
    collections::{HashMap, VecDeque},
    io,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;

use crate::types::{LogEvent, LogLevel, PodStatus, ConnectionState};

pub struct TuiApp {
    pub should_quit: bool,
    pub log_messages: VecDeque<LogEvent>,
    pub pod_status: HashMap<String, PodStatus>,
    pub scroll_offset: usize,
    pub paused: bool,
    pub show_help: bool,
    pub auto_scroll: bool,
    pub max_log_lines: usize,
    pub start_time: SystemTime,
    pub total_log_count: usize,
    pub gap_count: usize,
}

impl TuiApp {
    pub fn new(max_log_lines: usize, auto_scroll: bool) -> Self {
        Self {
            should_quit: false,
            log_messages: VecDeque::new(),
            pod_status: HashMap::new(),
            scroll_offset: 0,
            paused: false,
            show_help: false,
            auto_scroll,
            max_log_lines,
            start_time: SystemTime::now(),
            total_log_count: 0,
            gap_count: 0,
        }
    }

    pub fn add_log_event(&mut self, event: LogEvent) {
        match &event {
            LogEvent::Log(msg) => {
                self.total_log_count += 1;
                let pod_key = format!("{}/{}", msg.pod_name, msg.container_name);

                // Update pod status
                let status = self.pod_status.entry(pod_key).or_insert_with(|| PodStatus {
                    name: msg.pod_name.clone(),
                    container: msg.container_name.clone(),
                    last_log_time: Some(SystemTime::now()),
                    log_count: 0,
                    connection_state: ConnectionState::Connected,
                    color: convert_color(crate::utils::get_color(&msg.pod_name)),
                });
                status.last_log_time = Some(SystemTime::now());
                status.log_count += 1;
                status.connection_state = ConnectionState::Connected;
            }
            LogEvent::Gap { pod, container, .. } => {
                self.gap_count += 1;
                let pod_key = format!("{}/{}", pod, container);
                if let Some(status) = self.pod_status.get_mut(&pod_key) {
                    status.connection_state = ConnectionState::Disconnected {
                        since: SystemTime::now()
                    };
                }
            }
            LogEvent::StatusChange { pod, container, new_state, .. } => {
                let pod_key = format!("{}/{}", pod, container);
                if let Some(status) = self.pod_status.get_mut(&pod_key) {
                    status.connection_state = new_state.clone();
                }
            }
            LogEvent::SystemMessage { .. } => {}
        }

        // Add to log buffer
        self.log_messages.push_back(event);

        // Maintain buffer size
        while self.log_messages.len() > self.max_log_lines {
            self.log_messages.pop_front();
        }

        // Auto-scroll to bottom if enabled and not manually scrolled
        if self.auto_scroll && self.scroll_offset == 0 {
            self.scroll_to_end();
        }
    }

    pub fn draw(&mut self, f: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header
                Constraint::Length(8),  // Pod status table
                Constraint::Min(0),     // Log area
                Constraint::Length(1),  // Footer
            ])
            .split(f.area());

        self.draw_header(f, chunks[0]);
        self.draw_pod_status_table(f, chunks[1]);
        self.draw_log_area(f, chunks[2]);
        self.draw_footer(f, chunks[3]);

        if self.show_help {
            self.draw_help_popup(f);
        }
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let uptime = self.start_time.elapsed().unwrap_or_default();
        let uptime_str = format!("{:02}:{:02}:{:02}",
            uptime.as_secs() / 3600,
            (uptime.as_secs() % 3600) / 60,
            uptime.as_secs() % 60);

        let header_text = format!(
            "kubectl-tail | Pods: {} active | Total logs: {} | Gaps: {} | Uptime: {}",
            self.pod_status.len(),
            self.total_log_count,
            self.gap_count,
            uptime_str
        );

        let header = Paragraph::new(header_text)
            .block(Block::default()
                .borders(Borders::ALL)
                .title("Status")
                .title_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));

        f.render_widget(header, area);
    }

    fn draw_pod_status_table(&self, f: &mut Frame, area: Rect) {
        let header = Row::new(vec!["Status", "Pod/Container", "Last Log", "Lines", "State"])
            .style(Style::default().fg(Color::Yellow))
            .height(1);

        let rows: Vec<Row> = self.pod_status.values()
            .map(|status| {
                let (status_indicator, status_color) = match &status.connection_state {
                    ConnectionState::Connected => ("🟢", Color::Green),
                    ConnectionState::Reconnecting { attempt: _ } => ("🟡", Color::Yellow),
                    ConnectionState::Disconnected { .. } => ("🔴", Color::Red),
                    ConnectionState::Failed(_) => ("❌", Color::Red),
                };

                let last_log = status.last_log_time
                    .map(format_duration_since)
                    .unwrap_or_else(|| "Never".to_string());

                let state_text = match &status.connection_state {
                    ConnectionState::Connected => "Connected".to_string(),
                    ConnectionState::Reconnecting { attempt } => format!("Retry {}", attempt),
                    ConnectionState::Disconnected { since } => format!("Down {}s", since.elapsed().unwrap_or_default().as_secs()),
                    ConnectionState::Failed(err) => format!("Failed: {}", err),
                };

                Row::new(vec![
                    Cell::from(Span::styled(status_indicator, Style::default().fg(status_color))),
                    Cell::from(Span::styled(
                        format!("{}/{}", status.name, status.container),
                        Style::default().fg(status.color)
                    )),
                    Cell::from(last_log),
                    Cell::from(status.log_count.to_string()),
                    Cell::from(Span::styled(state_text, Style::default().fg(status_color))),
                ])
                .height(1)
            })
            .collect();

        let widths = [
            Constraint::Length(6),   // Status icon
            Constraint::Min(20),     // Pod/Container name
            Constraint::Length(12),  // Last log
            Constraint::Length(8),   // Line count
            Constraint::Min(15),     // State
        ];

        let table = Table::new(rows, widths)
            .header(header)
            .block(Block::default()
                .borders(Borders::ALL)
                .title("Pod Status"));

        f.render_widget(table, area);
    }

    fn draw_log_area(&self, f: &mut Frame, area: Rect) {
        let visible_height = area.height.saturating_sub(2) as usize; // Account for borders
        let total_messages = self.log_messages.len();
        let start_index = if total_messages <= visible_height {
            0
        } else {
            total_messages.saturating_sub(visible_height + self.scroll_offset)
        };

        let visible_messages: Vec<ListItem> = self.log_messages
            .iter()
            .skip(start_index)
            .take(visible_height)
            .map(|event| self.format_log_event(event))
            .collect();

        let title = if self.paused {
            "Logs (PAUSED - Press P to resume)"
        } else if self.scroll_offset > 0 {
            "Logs (Scrolled - Press End for live view)"
        } else {
            "Logs (Live)"
        };

        let list = List::new(visible_messages)
            .block(Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_style(if self.paused {
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Green)
                }));

        f.render_widget(list, area);
    }

    fn draw_footer(&self, f: &mut Frame, area: Rect) {
        let controls = if self.show_help {
            "Press F1 again to close help"
        } else {
            "F1: Help | P: Pause | ↑↓: Scroll | Home/End: Top/Bottom | Ctrl+C: Quit"
        };

        let footer = Paragraph::new(controls)
            .style(Style::default().fg(Color::Gray))
            .block(Block::default());

        f.render_widget(footer, area);
    }

    fn draw_help_popup(&self, f: &mut Frame) {
        let area = centered_rect(60, 70, f.area());

        let help_text = vec![
            Line::from(vec![Span::styled("kubectl-tail Controls", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
            Line::from(""),
            Line::from("Navigation:"),
            Line::from("  ↑/↓ or j/k     - Scroll up/down"),
            Line::from("  Page Up/Down   - Scroll by page"),
            Line::from("  Home           - Go to top"),
            Line::from("  End            - Go to bottom (live mode)"),
            Line::from(""),
            Line::from("Controls:"),
            Line::from("  p or F3        - Toggle pause/resume"),
            Line::from("  F1             - Toggle this help"),
            Line::from("  F4             - Save logs to file"),
            Line::from("  Ctrl+C or q    - Quit"),
            Line::from(""),
            Line::from("Status Indicators:"),
            Line::from("  🟢             - Pod connected"),
            Line::from("  🟡             - Pod reconnecting"),
            Line::from("  🔴             - Pod disconnected"),
            Line::from("  ❌             - Pod failed"),
            Line::from("  ⚠️              - Log gap detected"),
        ];

        let help_paragraph = Paragraph::new(help_text)
            .block(Block::default()
                .title("Help")
                .borders(Borders::ALL)
                .style(Style::default().bg(Color::Black)))
            .style(Style::default().fg(Color::White));

        f.render_widget(Clear, area); // Clear background
        f.render_widget(help_paragraph, area);
    }

    fn format_log_event(&self, event: &LogEvent) -> ListItem<'_> {
        match event {
            LogEvent::Log(msg) => {
                let timestamp = format_timestamp(msg.timestamp);
                let pod_key = format!("{}/{}", msg.pod_name, msg.container_name);
                let pod_color = self.pod_status.get(&pod_key)
                    .map(|s| s.color)
                    .unwrap_or(Color::White);

                let level_style = match msg.level {
                    Some(LogLevel::Error) => Style::default().fg(Color::Red),
                    Some(LogLevel::Warning) => Style::default().fg(Color::Yellow),
                    Some(LogLevel::Info) => Style::default().fg(Color::Blue),
                    Some(LogLevel::Debug) => Style::default().fg(Color::Gray),
                    None => Style::default(),
                };

                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(timestamp, Style::default().fg(Color::Gray)),
                        Span::raw(" "),
                        Span::styled(
                            format!("[{}/{}]", msg.pod_name, msg.container_name),
                            Style::default().fg(pod_color).add_modifier(Modifier::BOLD)
                        ),
                        Span::raw(" "),
                        Span::styled(msg.line.clone(), level_style),
                    ])
                ])
            }
            LogEvent::Gap { duration, reason, pod, container } => {
                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled("⚠️  LOG GAP DETECTED",
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                        Span::styled(format!(" - {}/{} missing {:.1}s of logs ({:?})",
                            pod, container, duration.as_secs_f32(), reason),
                            Style::default().fg(Color::Yellow)),
                    ])
                ])
            }
            LogEvent::StatusChange { pod, container, new_state, .. } => {
                let (icon, color) = match new_state {
                    ConnectionState::Connected => ("🔄➡️🟢", Color::Green),
                    ConnectionState::Reconnecting { .. } => ("🔄", Color::Yellow),
                    ConnectionState::Disconnected { .. } => ("🟢➡️🔴", Color::Red),
                    ConnectionState::Failed(_) => ("❌", Color::Red),
                };

                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(icon, Style::default().fg(color)),
                        Span::styled(format!(" {}/{} {}", pod, container, format_connection_state(new_state)),
                            Style::default().fg(color)),
                    ])
                ])
            }
            LogEvent::SystemMessage { level, message } => {
                let (icon, color) = match level {
                    LogLevel::Info => ("ℹ️", Color::Blue),
                    LogLevel::Warning => ("⚠️", Color::Yellow),
                    LogLevel::Error => ("❌", Color::Red),
                    LogLevel::Debug => ("🔍", Color::Gray),
                };

                ListItem::new(vec![
                    Line::from(vec![
                        Span::styled(icon, Style::default().fg(color)),
                        Span::styled(format!(" {}", message), Style::default().fg(color)),
                    ])
                ])
            }
        }
    }

    pub async fn handle_events(&mut self) -> anyhow::Result<()> {
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            self.should_quit = true;
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.should_quit = true;
                        }
                        KeyCode::Char('p') | KeyCode::F(3) => {
                            self.paused = !self.paused;
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            self.scroll_up();
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            self.scroll_down();
                        }
                        KeyCode::PageUp => {
                            self.page_up();
                        }
                        KeyCode::PageDown => {
                            self.page_down();
                        }
                        KeyCode::Home => {
                            self.scroll_offset = self.log_messages.len().saturating_sub(1);
                        }
                        KeyCode::End => {
                            self.scroll_offset = 0;
                        }
                        KeyCode::F(1) => {
                            self.show_help = !self.show_help;
                        }
                        KeyCode::F(4) => {
                            // TODO: Implement save logs functionality
                        }
                        _ => {}
                    }
                }
        Ok(())
    }

    fn scroll_up(&mut self) {
        if self.scroll_offset < self.log_messages.len().saturating_sub(1) {
            self.scroll_offset += 1;
        }
    }

    fn scroll_down(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
        }
    }

    fn page_up(&mut self) {
        self.scroll_offset = (self.scroll_offset + 20).min(self.log_messages.len().saturating_sub(1));
    }

    fn page_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(20);
    }

    fn scroll_to_end(&mut self) {
        self.scroll_offset = 0;
    }
}

pub async fn run_tui(
    max_log_lines: usize,
    auto_scroll: bool,
    mut event_rx: mpsc::Receiver<LogEvent>,
) -> anyhow::Result<()> {
    // Verify we're in a proper terminal environment
    if !atty::is(atty::Stream::Stdout) {
        return Err(anyhow::anyhow!("TUI mode requires a terminal (stdout is not a tty)"));
    }

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create TUI app
    let mut app = TuiApp::new(max_log_lines, auto_scroll);

    // Main loop
    let result = loop {
        terminal.draw(|f| app.draw(f))?;

        // Handle UI events
        if let Err(e) = app.handle_events().await {
            break Err(e);
        }

        if app.should_quit {
            break Ok(());
        }

        // Handle new log events
        while let Ok(event) = event_rx.try_recv() {
            if !app.paused {
                app.add_log_event(event);
            }
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    result
}

// Helper functions
fn format_duration_since(time: SystemTime) -> String {
    match time.elapsed() {
        Ok(duration) => {
            let secs = duration.as_secs();
            if secs < 60 {
                format!("{}s ago", secs)
            } else if secs < 3600 {
                format!("{}m ago", secs / 60)
            } else {
                format!("{}h ago", secs / 3600)
            }
        }
        Err(_) => "Unknown".to_string(),
    }
}

fn format_timestamp(time: SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let secs = duration.as_secs();
            let hours = (secs / 3600) % 24;
            let minutes = (secs / 60) % 60;
            let seconds = secs % 60;
            format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
        }
        Err(_) => "??:??:??".to_string(),
    }
}

fn format_connection_state(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Connected => "connected".to_string(),
        ConnectionState::Reconnecting { attempt } => format!("reconnecting (attempt {})", attempt),
        ConnectionState::Disconnected { since } => {
            format!("disconnected for {}s", since.elapsed().unwrap_or_default().as_secs())
        }
        ConnectionState::Failed(err) => format!("failed: {}", err),
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

// Helper function to convert crossterm color to ratatui color
fn convert_color(color: crossterm::style::Color) -> Color {
    match color {
        crossterm::style::Color::Red => Color::Red,
        crossterm::style::Color::Green => Color::Green,
        crossterm::style::Color::Blue => Color::Blue,
        crossterm::style::Color::Yellow => Color::Yellow,
        crossterm::style::Color::Magenta => Color::Magenta,
        crossterm::style::Color::Cyan => Color::Cyan,
        crossterm::style::Color::White => Color::White,
        crossterm::style::Color::Grey => Color::Gray,
        crossterm::style::Color::AnsiValue(v) => Color::Indexed(v),
        _ => Color::White,
    }
}