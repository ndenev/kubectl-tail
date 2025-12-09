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

    /// Context
    #[arg(long)]
    pub context: Option<String>,

    /// Number of lines to show from the end of the logs on startup
    #[arg(long)]
    pub tail: Option<i64>,

    /// Enable verbose output
    #[arg(short, long)]
    pub verbose: bool,

    /// Use plain text output instead of TUI (useful for piping)
    #[arg(long)]
    pub plain_output: bool,

    /// Maximum number of log lines to keep in memory for TUI mode
    #[arg(long, default_value = "10000")]
    pub buffer_size: usize,

    /// Auto-scroll to follow new logs in TUI mode
    #[arg(long, default_value = "true")]
    pub auto_scroll: bool,
}
