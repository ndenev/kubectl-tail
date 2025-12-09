use std::time::{Duration, SystemTime};
use ratatui::style::Color;

#[derive(Debug, Clone)]
pub struct LogMessage {
    pub pod_name: String,
    pub container_name: String,
    pub line: String,
    pub timestamp: SystemTime,
    pub level: Option<LogLevel>,
}

#[derive(Debug, Clone)]
pub enum LogEvent {
    Log(LogMessage),
    Gap { duration: Duration, reason: GapReason, pod: String, container: String },
    StatusChange { pod: String, container: String, #[allow(dead_code)] old_state: ConnectionState, new_state: ConnectionState },
    #[allow(dead_code)]
    SystemMessage { level: LogLevel, message: String },
}

#[derive(Debug, Clone)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
    Debug,
}

#[derive(Debug, Clone)]
pub enum GapReason {
    #[allow(dead_code)]
    NetworkError(String),
    #[allow(dead_code)]
    ApiServerError(u16),
    StreamEnded,
    #[allow(dead_code)]
    PodNotFound,
}

#[derive(Debug, Clone)]
pub struct PodStatus {
    pub name: String,
    pub container: String,
    pub last_log_time: Option<SystemTime>,
    pub log_count: usize,
    pub connection_state: ConnectionState,
    pub color: Color,
}

#[derive(Debug, Clone)]
pub enum ConnectionState {
    Connected,
    Reconnecting { attempt: u32 },
    Disconnected { since: SystemTime },
    Failed(String),
}

impl LogMessage {
    pub fn new(pod_name: String, container_name: String, line: String) -> Self {
        let level = detect_log_level(&line);
        Self {
            pod_name,
            container_name,
            line,
            timestamp: SystemTime::now(),
            level,
        }
    }
}

fn detect_log_level(line: &str) -> Option<LogLevel> {
    let line_upper = line.to_uppercase();
    if line_upper.contains("ERROR") || line_upper.contains("FATAL") {
        Some(LogLevel::Error)
    } else if line_upper.contains("WARN") {
        Some(LogLevel::Warning)
    } else if line_upper.contains("INFO") {
        Some(LogLevel::Info)
    } else if line_upper.contains("DEBUG") || line_upper.contains("TRACE") {
        Some(LogLevel::Debug)
    } else {
        None
    }
}
