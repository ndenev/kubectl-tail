#[derive(Debug, Clone)]
pub struct LogMessage {
    pub cluster: String,
    pub namespace: String,
    pub pod_name: String,
    pub container_name: String,
    pub line: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}
