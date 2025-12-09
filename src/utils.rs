use crossterm::style::Color;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::core::Selector as KubeSelector;
use std::hash::{Hash, Hasher};

/// Render a Kubernetes `LabelSelector` to an API-ready string using kube's typed selector
/// handling. Returns `None` if the selector is empty or cannot be converted.
pub fn selector_to_labels_string(selector: &LabelSelector) -> Option<String> {
    KubeSelector::try_from(selector.clone())
        .ok()
        .filter(|s| !s.selects_all())
        .map(|s| s.to_string())
}

/// Generate a color for a string based on hash.
pub fn get_color(s: &str) -> Color {
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
