#[derive(Debug, Clone)]
pub struct LogMessage {
    pub cluster: String,
    pub namespace: String,
    pub pod_name: String,
    pub container_name: String,
    pub line: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct ResourceSpec {
    pub context: Option<String>,
    pub namespace: Option<String>,
    pub kind: Option<String>,
    pub name: String,
}
