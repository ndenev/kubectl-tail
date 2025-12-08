#[derive(Debug)]
pub struct LogMessage {
    pub pod_name: String,
    pub container_name: String,
    pub line: String,
}