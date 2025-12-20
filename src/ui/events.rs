use crate::types::LogMessage;
use crate::ui::app::{App, AppMode, PodInfo, PodKey};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use tokio::sync::mpsc;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    LogMessage(LogMessage),
    PodUpdate(PodUpdateEvent),
    Tick,
    #[allow(dead_code)]
    Quit,
}

#[derive(Debug)]
pub struct PodUpdateEvent {
    pub info: PodInfo,
    pub event_type: PodEventType,
}

#[derive(Debug)]
pub enum PodEventType {
    Added,
    #[allow(dead_code)]
    Updated,
    Deleted(PodKey),
}

pub async fn event_loop(tx: mpsc::Sender<AppEvent>) {
    use crossterm::event::EventStream;

    let mut event_stream = EventStream::new();
    let mut tick_interval = tokio::time::interval(std::time::Duration::from_millis(250));

    loop {
        tokio::select! {
            maybe_event = event_stream.next() => {
                if let Some(Ok(Event::Key(key))) = maybe_event
                    && tx.send(AppEvent::Key(key)).await.is_err() {
                        break;
                    }
            }
            _ = tick_interval.tick() => {
                if tx.send(AppEvent::Tick).await.is_err() {
                    break;
                }
            }
        }
    }
}

pub fn handle_key_event(app: &mut App, key: KeyEvent) -> bool {
    match app.mode {
        AppMode::Normal => handle_normal_mode(app, key),
        AppMode::Search => handle_search_mode(app, key),
        AppMode::Filter => handle_filter_mode(app, key),
        AppMode::Help => handle_help_mode(app, key),
    }
}

fn handle_normal_mode(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), _)
        | (KeyCode::Char('Q'), _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            return false;
        }
        (KeyCode::Char('s'), _) => {
            app.sidebar_visible = !app.sidebar_visible;
        }
        (KeyCode::Char('p'), _) => {
            app.paused = !app.paused;
        }
        (KeyCode::Char('c'), _) => {
            app.clear_logs();
        }
        (KeyCode::Char('a'), _) => {
            app.auto_scroll = !app.auto_scroll;
        }
        (KeyCode::Char('t'), _) => {
            app.show_timestamps = !app.show_timestamps;
        }
        (KeyCode::Char('x'), _) => {
            app.show_prefix = !app.show_prefix;
        }
        (KeyCode::Char('/'), _) => {
            app.mode = AppMode::Search;
            app.search_pattern.clear();
        }
        (KeyCode::Char('f'), _) => {
            // Start filter input mode
            app.mode = AppMode::Filter;
            app.filter_pattern.clear();
        }
        (KeyCode::Char('?'), _) => {
            app.help_visible = !app.help_visible;
            if app.help_visible {
                app.mode = AppMode::Help;
            }
        }
        (KeyCode::Char('n'), _) => {
            app.jump_to_next_match();
        }
        (KeyCode::Char('N'), _) => {
            app.jump_to_prev_match();
        }
        (KeyCode::Up, _) => {
            if app.sidebar_visible && app.sidebar_state.selected().is_some() {
                app.sidebar_select_previous();
            } else {
                app.scroll_up();
            }
        }
        (KeyCode::Down, _) => {
            if app.sidebar_visible && app.sidebar_state.selected().is_some() {
                app.sidebar_select_next();
            } else {
                app.scroll_down();
            }
        }
        (KeyCode::PageUp, _) => {
            app.page_up(20);
        }
        (KeyCode::PageDown, _) => {
            app.page_down(20);
        }
        (KeyCode::Home, _) => {
            app.scroll_to_top();
        }
        (KeyCode::End, _) => {
            app.scroll_to_bottom();
        }
        (KeyCode::Char(' '), _) if app.sidebar_visible => {
            app.toggle_pod();
        }
        _ => {}
    }
    true
}

fn handle_search_mode(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            app.search_pattern.clear();
            app.search_matches.clear();
        }
        KeyCode::Enter => {
            app.mode = AppMode::Normal;
            app.update_search_matches();
        }
        KeyCode::Char(c) => {
            app.search_pattern.push(c);
        }
        KeyCode::Backspace => {
            app.search_pattern.pop();
        }
        _ => {}
    }
    true
}

fn handle_filter_mode(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            app.filter_pattern.clear();
        }
        KeyCode::Enter => {
            app.mode = AppMode::Normal;
            // Filter is applied automatically in filtered_logs()
        }
        KeyCode::Char(c) => {
            app.filter_pattern.push(c);
        }
        KeyCode::Backspace => {
            app.filter_pattern.pop();
        }
        _ => {}
    }
    true
}

fn handle_help_mode(app: &mut App, _key: KeyEvent) -> bool {
    app.help_visible = false;
    app.mode = AppMode::Normal;
    true
}
