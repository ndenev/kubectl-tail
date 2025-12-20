use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub struct AppLayout {
    pub sidebar: Rect,
    pub main: Rect,
    pub status_bar: Rect,
}

pub fn create_layout(area: Rect, sidebar_visible: bool) -> AppLayout {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // Main area
            Constraint::Length(1), // Status bar
        ])
        .split(area);

    if sidebar_visible {
        let horizontal = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(60), // Sidebar
                Constraint::Min(1),     // Logs
            ])
            .split(main_chunks[0]);

        AppLayout {
            sidebar: horizontal[0],
            main: horizontal[1],
            status_bar: main_chunks[1],
        }
    } else {
        AppLayout {
            sidebar: Rect::default(),
            main: main_chunks[0],
            status_bar: main_chunks[1],
        }
    }
}
