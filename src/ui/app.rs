use crate::types::LogMessage;
use ratatui::widgets::ListState;
use regex::Regex;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PodKey {
    pub cluster: String,
    pub namespace: String,
    pub pod_name: String,
    pub container_name: String,
}

#[derive(Debug, Clone)]
pub struct PodInfo {
    pub key: PodKey,
    pub phase: String,
    #[allow(dead_code)]
    pub age: String,
    #[allow(dead_code)]
    pub restarts: i32,
}

pub struct PodState {
    pub enabled: bool,
    #[allow(dead_code)]
    pub last_seen: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AppMode {
    Normal,
    Search,
    Filter,
    Help,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TreeNodeType {
    Cluster(String),
    Namespace(String, String), // cluster, namespace
    Pod(String, String, String), // cluster, namespace, pod
    Container, // Leaf node
}

pub struct App {
    // Log buffer - ring buffer with configurable size
    pub log_buffer: VecDeque<LogMessage>,
    pub max_buffer_size: usize,

    // Pod tracking
    pub pods: Vec<PodInfo>,
    pub pod_states: HashMap<PodKey, PodState>,

    // UI state
    pub sidebar_visible: bool,
    pub sidebar_state: ListState,
    pub sidebar_item_keys: Vec<Option<PodKey>>, // Maps list index to container key (None for headers)
    pub sidebar_item_types: Vec<TreeNodeType>, // Type of each item for collapse/expand
    pub expanded_nodes: std::collections::HashSet<String>, // Set of expanded node paths
    pub scroll_offset: usize,
    pub auto_scroll: bool,

    // Search state (/ key - highlights and allows n/N navigation)
    pub search_pattern: String,
    pub search_matches: Vec<usize>,
    pub current_match_index: usize,

    // Filter state (f key - shows only matching lines)
    pub filter_pattern: String,
    pub active_filters: Vec<String>,

    // Status tracking
    pub running_pods: usize,
    pub total_pods: usize,
    pub memory_usage: usize,

    // UI mode
    pub mode: AppMode,
    pub help_visible: bool,
    pub paused: bool,
    pub show_timestamps: bool,
    pub show_prefix: bool,
}

impl App {
    pub fn new(max_buffer_size: usize) -> Self {
        Self {
            log_buffer: VecDeque::with_capacity(max_buffer_size),
            max_buffer_size,
            pods: Vec::new(),
            pod_states: HashMap::new(),
            sidebar_visible: false,
            sidebar_state: ListState::default(),
            sidebar_item_keys: Vec::new(),
            sidebar_item_types: Vec::new(),
            expanded_nodes: std::collections::HashSet::new(),
            scroll_offset: 0,
            auto_scroll: true,
            search_pattern: String::new(),
            search_matches: Vec::new(),
            current_match_index: 0,
            filter_pattern: String::new(),
            active_filters: Vec::new(),
            running_pods: 0,
            total_pods: 0,
            memory_usage: 0,
            mode: AppMode::Normal,
            help_visible: false,
            paused: false,
            show_timestamps: false,
            show_prefix: true,
        }
    }

    pub fn add_log(&mut self, msg: LogMessage) {
        if !self.paused {
            // Enforce ring buffer size
            while self.log_buffer.len() >= self.max_buffer_size {
                self.log_buffer.pop_front();
                // Adjust scroll offset if we removed from the front
                if self.scroll_offset > 0 {
                    self.scroll_offset = self.scroll_offset.saturating_sub(1);
                }
            }

            self.log_buffer.push_back(msg);

            // Auto-scroll to bottom if enabled
            if self.auto_scroll {
                self.scroll_offset = self.log_buffer.len().saturating_sub(1);
            }
        }
    }

    pub fn add_pod(&mut self, info: PodInfo) {
        // Add pod state if not exists
        if !self.pod_states.contains_key(&info.key) {
            self.pod_states.insert(
                info.key.clone(),
                PodState {
                    enabled: true,
                    last_seen: chrono::Utc::now(),
                },
            );
        }

        // Update pod info
        if let Some(existing) = self.pods.iter_mut().find(|p| p.key == info.key) {
            *existing = info;
        } else {
            self.pods.push(info.clone());

            // Auto-expand parent nodes for new pods
            self.expanded_nodes.insert(info.key.cluster.clone());
            self.expanded_nodes.insert(format!(
                "{}/{}",
                info.key.cluster, info.key.namespace
            ));
            self.expanded_nodes.insert(format!(
                "{}/{}/{}",
                info.key.cluster, info.key.namespace, info.key.pod_name
            ));

            // Auto-select first pod if sidebar visible and nothing selected
            if self.sidebar_visible
                && self.sidebar_state.selected().is_none()
                && self.pods.len() == 1
            {
                self.sidebar_state.select(Some(0));
            }
        }
    }

    pub fn remove_pod(&mut self, key: &PodKey) {
        self.pods.retain(|p| &p.key != key);
        self.pod_states.remove(key);
    }

    pub fn toggle_sidebar_item(&mut self) {
        if let Some(idx) = self.sidebar_state.selected()
            && let Some(node_type) = self.sidebar_item_types.get(idx)
        {
            match node_type {
                TreeNodeType::Cluster(name) => {
                    // Toggle expand/collapse
                    let path = name.clone();
                    if self.expanded_nodes.contains(&path) {
                        self.expanded_nodes.remove(&path);
                    } else {
                        self.expanded_nodes.insert(path);
                    }
                }
                TreeNodeType::Namespace(cluster, namespace) => {
                    let path = format!("{}/{}", cluster, namespace);
                    if self.expanded_nodes.contains(&path) {
                        self.expanded_nodes.remove(&path);
                    } else {
                        self.expanded_nodes.insert(path);
                    }
                }
                TreeNodeType::Pod(cluster, namespace, pod) => {
                    let path = format!("{}/{}/{}", cluster, namespace, pod);
                    if self.expanded_nodes.contains(&path) {
                        self.expanded_nodes.remove(&path);
                    } else {
                        self.expanded_nodes.insert(path);
                    }
                }
                TreeNodeType::Container => {
                    // Toggle container enabled/disabled
                    if let Some(Some(key)) = self.sidebar_item_keys.get(idx)
                        && let Some(state) = self.pod_states.get_mut(key)
                    {
                        state.enabled = !state.enabled;
                    }
                }
            }
        }
    }

    pub fn filtered_logs(&self) -> Vec<&LogMessage> {
        let filter_regex = if !self.filter_pattern.is_empty() {
            // Make filter case-insensitive by default (prepend (?i))
            let pattern = format!("(?i){}", self.filter_pattern);
            Regex::new(&pattern).ok()
        } else {
            None
        };

        self.log_buffer
            .iter()
            .filter(|msg| {
                // Check if pod is enabled
                let key = PodKey {
                    cluster: msg.cluster.clone(),
                    namespace: msg.namespace.clone(),
                    pod_name: msg.pod_name.clone(),
                    container_name: msg.container_name.clone(),
                };
                let enabled = self.pod_states.get(&key).map(|s| s.enabled).unwrap_or(true);

                if !enabled {
                    return false;
                }

                // Apply filter pattern (f key - shows only matching lines)
                if let Some(ref re) = filter_regex {
                    return re.is_match(&msg.line);
                }

                true
            })
            .collect()
    }

    pub fn update_search_matches(&mut self) {
        self.search_matches.clear();

        if self.search_pattern.is_empty() {
            return;
        }

        // Make search case-insensitive by default (prepend (?i))
        let pattern = format!("(?i){}", self.search_pattern);
        let Ok(regex) = Regex::new(&pattern) else {
            return;
        };

        // Search within filtered logs (respects filter_pattern and pod toggles)
        let filtered = self.filtered_logs();
        self.search_matches = filtered
            .iter()
            .enumerate()
            .filter(|(_, msg)| regex.is_match(&msg.line))
            .map(|(idx, _)| idx)
            .collect();

        self.current_match_index = 0;
    }

    pub fn jump_to_next_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.current_match_index = (self.current_match_index + 1) % self.search_matches.len();
        self.scroll_offset = self.search_matches[self.current_match_index];
        self.auto_scroll = false;
    }

    pub fn jump_to_prev_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        if self.current_match_index == 0 {
            self.current_match_index = self.search_matches.len() - 1;
        } else {
            self.current_match_index -= 1;
        }
        self.scroll_offset = self.search_matches[self.current_match_index];
        self.auto_scroll = false;
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
        self.auto_scroll = false;
    }

    pub fn scroll_down(&mut self) {
        if self.scroll_offset < self.log_buffer.len().saturating_sub(1) {
            self.scroll_offset += 1;
        } else {
            self.auto_scroll = true;
        }
    }

    pub fn page_up(&mut self, page_size: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(page_size);
        self.auto_scroll = false;
    }

    pub fn page_down(&mut self, page_size: usize) {
        let max_offset = self.log_buffer.len().saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + page_size).min(max_offset);
        if self.scroll_offset >= max_offset {
            self.auto_scroll = true;
        }
    }

    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = false;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = self.log_buffer.len().saturating_sub(1);
        self.auto_scroll = true;
    }

    pub fn sidebar_select_next(&mut self) {
        if self.sidebar_item_keys.is_empty() {
            return;
        }
        let i = match self.sidebar_state.selected() {
            Some(i) => {
                if i >= self.sidebar_item_keys.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.sidebar_state.select(Some(i));
    }

    pub fn sidebar_select_previous(&mut self) {
        if self.sidebar_item_keys.is_empty() {
            return;
        }
        let i = match self.sidebar_state.selected() {
            Some(i) => {
                if i == 0 {
                    self.sidebar_item_keys.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.sidebar_state.select(Some(i));
    }

    pub fn update_stats(&mut self) {
        // Count running/enabled pods
        self.running_pods = self.pod_states.values().filter(|s| s.enabled).count();
        self.total_pods = self.pod_states.len();

        // Estimate memory usage (approximate: 200 bytes per log line)
        let avg_line_size = 200;
        self.memory_usage = (self.log_buffer.len() * avg_line_size) / (1024 * 1024);

        // Update active filters
        self.active_filters.clear();
        if !self.filter_pattern.is_empty() {
            self.active_filters
                .push(format!("filter: {}", self.filter_pattern));
        }
        if !self.search_pattern.is_empty() {
            self.active_filters
                .push(format!("search: {}", self.search_pattern));
        }
        if self.paused {
            self.active_filters.push("PAUSED".to_string());
        }
    }

    pub fn clear_logs(&mut self) {
        self.log_buffer.clear();
        self.scroll_offset = 0;
        self.search_matches.clear();
    }

    pub fn get_clusters(&self) -> Vec<String> {
        self.pod_states
            .keys()
            .map(|k| k.cluster.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }
}
