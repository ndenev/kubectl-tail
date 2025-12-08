use clap::Parser;

#[derive(Parser)]
#[command(name = "kubectl-tail")]
#[command(about = "Tail logs from Kubernetes pods with continuous discovery")]
pub struct Cli {
    /// Resource to tail logs from (e.g., my-pod, deployment/my-deployment)
    pub resource: Option<String>,

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
}
