use crossterm::style::Color;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use std::hash::{Hash, Hasher};

/// Parse a label selector string into a BTreeMap.
pub fn parse_labels(sel_str: &str) -> std::collections::BTreeMap<String, String> {
    let mut map = std::collections::BTreeMap::new();
    for pair in sel_str.split(',') {
        let pair = pair.trim();
        if let Some(eq_pos) = pair.find('=') {
            let key = pair[..eq_pos].to_string();
            let value = pair[eq_pos + 1..].to_string();
            map.insert(key, value);
        }
    }
    map
}

/// Convert a LabelSelector's match_labels to a string for listing.
pub fn selector_to_labels_string(selector: &LabelSelector) -> Option<String> {
    if let Some(labels) = &selector.match_labels {
        if labels.is_empty() {
            None
        } else {
            Some(
                labels
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(","),
            )
        }
    } else {
        None
    }
}

/// Check if pod labels match the given LabelSelector.
pub fn matches_selector(
    pod_labels: &std::collections::BTreeMap<String, String>,
    selector: &LabelSelector,
) -> bool {
    // Check match_labels
    if let Some(match_labels) = &selector.match_labels {
        for (key, value) in match_labels {
            if pod_labels.get(key) != Some(value) {
                return false;
            }
        }
    }
    // Check match_expressions
    if let Some(expressions) = &selector.match_expressions {
        for expr in expressions {
            let pod_value = pod_labels.get(&expr.key);
            match expr.operator.as_str() {
                "In" => {
                    if let Some(values) = &expr.values {
                        if pod_value.is_none() || !values.contains(pod_value.unwrap()) {
                            return false;
                        }
                    } else {
                        return false; // No values for In
                    }
                }
                "NotIn" => {
                    if let Some(values) = &expr.values {
                        if pod_value.is_some() && values.contains(pod_value.unwrap()) {
                            return false;
                        }
                    } else {
                        return false; // No values for NotIn
                    }
                }
                "Exists" => {
                    if pod_value.is_none() {
                        return false;
                    }
                }
                "DoesNotExist" => {
                    if pod_value.is_some() {
                        return false;
                    }
                }
                _ => return false, // Unknown operator
            }
        }
    }
    true
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
