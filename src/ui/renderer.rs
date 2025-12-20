use crate::ui::app::App;
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
        let pod_list = PodList::new(&app.pods, &app.pod_states);
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
            widgets::{Block, Borders, Paragraph},
        };

        let search_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(f.area())[1];

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
            widgets::{Block, Borders, Paragraph},
        };

        let filter_area = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(f.area())[1];

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
