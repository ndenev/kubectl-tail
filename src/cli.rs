use clap::Parser;

#[derive(Parser)]
#[command(name = "kubectl-tail")]
#[command(about = "Tail logs from Kubernetes pods with continuous discovery")]
pub struct Cli {
    /// Resources to tail logs from (e.g., my-pod, deployment/my-deployment)
    pub resources: Vec<String>,

    /// Label selector
    #[arg(short = 'l', long)]
    pub selector: Option<String>,

    /// Namespace
    #[arg(short = 'n', long)]
    pub namespace: Option<String>,

    /// Container name (if multi-container pod)
    #[arg(short = 'c', long)]
    pub container: Option<String>,

    /// Kubernetes context (single value - for multi-cluster use context/namespace/kind/name format)
    #[arg(long = "context")]
    pub context: Option<String>,

    /// Number of lines to show from the end of the logs on startup
    #[arg(long)]
    pub tail: Option<i64>,

    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Filter logs by regex pattern
    #[arg(short = 'g', long)]
    pub grep: Option<String>,

    /// Disable TUI mode and use stdout (backward compatibility)
    #[arg(long)]
    pub no_tui: bool,

    /// Maximum buffer size for log messages in TUI mode
    #[arg(long, default_value = "10000")]
    pub buffer_size: usize,
}
