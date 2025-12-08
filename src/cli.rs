use clap::Parser;

#[derive(Parser)]
#[command(name = "kubectl-tail")]
#[command(about = "Tail logs from Kubernetes pods with continuous discovery")]
pub struct Cli {
    /// Resource type (pod, deployment, statefulset, daemonset, job, etc.)
    #[arg(short, long, default_value = "pod")]
    pub resource_type: String,

    /// Resource name (if specifying a specific resource)
    pub resource_name: Option<String>,

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