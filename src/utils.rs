use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::core::Selector as KubeSelector;
use ratatui::style::Color;
use std::hash::{Hash, Hasher};

/// Render a Kubernetes `LabelSelector` to an API-ready string using kube's typed selector
/// handling. Returns `None` if the selector is empty or cannot be converted.
pub fn selector_to_labels_string(selector: &LabelSelector) -> Option<String> {
    KubeSelector::try_from(selector.clone())
        .ok()
        .filter(|s| !s.selects_all())
        .map(|s| s.to_string())
}

/// Generate a ratatui color for a string based on hash (for TUI mode).
pub fn get_color(s: &str) -> Color {
    let colors = [
        Color::Red,
        Color::Green,
        Color::Blue,
        Color::Yellow,
        Color::Magenta,
        Color::Cyan,
        Color::White,
        Color::Gray,
        Color::LightRed,
        Color::LightGreen,
        Color::LightBlue,
        Color::LightYellow,
        Color::LightMagenta,
        Color::LightCyan,
    ];
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    let hash = hasher.finish() as u32;
    colors[(hash % colors.len() as u32) as usize]
}

/// Generate a crossterm color for a string based on hash (for stdout mode).
pub fn get_crossterm_color(s: &str) -> crossterm::style::Color {
    let colors = [
        crossterm::style::Color::Red,
        crossterm::style::Color::Green,
        crossterm::style::Color::Blue,
        crossterm::style::Color::Yellow,
        crossterm::style::Color::Magenta,
        crossterm::style::Color::Cyan,
        crossterm::style::Color::White,
        crossterm::style::Color::Grey,
        crossterm::style::Color::AnsiValue(91), // Bright Red
        crossterm::style::Color::AnsiValue(92), // Bright Green
        crossterm::style::Color::AnsiValue(94), // Bright Blue
        crossterm::style::Color::AnsiValue(93), // Bright Yellow
        crossterm::style::Color::AnsiValue(95), // Bright Magenta
        crossterm::style::Color::AnsiValue(96), // Bright Cyan
    ];
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    let hash = hasher.finish() as u32;
    colors[(hash % colors.len() as u32) as usize]
}
