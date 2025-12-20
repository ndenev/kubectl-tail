use crate::types::ResourceSpec;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use kube::core::Selector as KubeSelector;
use ratatui::style::Color;
use regex::Regex;
use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

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

/// Parse a resource specification in format:
/// - context/namespace/kind/name (4 parts)
/// - namespace/kind/name (3 parts)
/// - kind/name (2 parts)
/// - name (1 part - assumed to be a pod)
pub fn parse_resource_spec(spec: &str) -> Result<ResourceSpec, String> {
    let parts: Vec<&str> = spec.split('/').collect();

    match parts.len() {
        1 => {
            // Just a name - assumed to be a pod
            Ok(ResourceSpec {
                context: None,
                namespace: None,
                kind: None,
                name: parts[0].to_string(),
            })
        }
        2 => {
            // kind/name
            Ok(ResourceSpec {
                context: None,
                namespace: None,
                kind: Some(parts[0].to_string()),
                name: parts[1].to_string(),
            })
        }
        3 => {
            // namespace/kind/name
            Ok(ResourceSpec {
                context: None,
                namespace: Some(parts[0].to_string()),
                kind: Some(parts[1].to_string()),
                name: parts[2].to_string(),
            })
        }
        4 => {
            // context/namespace/kind/name
            Ok(ResourceSpec {
                context: Some(parts[0].to_string()),
                namespace: Some(parts[1].to_string()),
                kind: Some(parts[2].to_string()),
                name: parts[3].to_string(),
            })
        }
        _ => Err(format!(
            "Invalid resource spec '{}': expected 1-4 parts separated by '/'",
            spec
        )),
    }
}

/// Strip ANSI escape codes from a string
/// Uses a cached regex for performance
pub fn strip_ansi_codes(s: &str) -> String {
    static ANSI_REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = ANSI_REGEX.get_or_init(|| {
        // This regex matches ANSI escape sequences:
        // \x1b is the ESC character
        // Matches CSI sequences: \x1b\[[0-9;]*[a-zA-Z]
        // Matches OSC sequences: \x1b\][^\x07]*\x07
        // And other escape sequences
        Regex::new(r"\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])").unwrap()
    });
    regex.replace_all(s, "").to_string()
}
