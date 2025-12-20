# kubectl-tail

A kubectl plugin for tailing logs from Kubernetes pods with continuous discovery.

## Features

- **Interactive TUI Mode** with minimalist terminal interface (default)
  - Borderless, full-screen log view with optional sidebar (press `s`)
  - Sidebar uses simple vertical line divider when visible
  - Real-time log streaming with color-coded prefixes
  - Search with highlighting (`/`) and n/N navigation
  - Separate filter (`f`) to show only matching lines (works on buffer)
  - Pod/container toggling to control which logs to display (in sidebar)
  - Status bar showing stats and help hint ("? for help")
  - Help overlay with keyboard shortcuts (press `?`, fully opaque for easy reading)
- **Multi-cluster support** - tail logs across multiple Kubernetes clusters simultaneously
- Tail logs from pods with automatic discovery of new pods
- Support for multiple resources (e.g., several deployments) and various resource types (pods, deployments, statefulsets, daemonsets, jobs, etc.)
- Filter by namespace, labels, and resource names
- Colorized output for easy log differentiation
- Continuous monitoring with graceful handling of pod restarts and deletions
- Memory-bounded ring buffer to prevent unbounded growth
- Backward compatible stdout mode (`--no-tui` flag)
- Requires at least one resource or label selector to prevent accidental whole-namespace tailing

## Installation

### Prerequisites

- Rust (latest stable version)
- kubectl configured to access your Kubernetes cluster

### Installation

#### Quick Installation (via Cargo)

Install directly from the GitHub repository:

```bash
cargo install --git https://github.com/ndenev/kubectl-tail.git
```

Make sure `~/.cargo/bin` is in your PATH.

#### Manual Installation

1. Clone the repository:
   ```bash
   git clone https://github.com/ndenev/kubectl-tail.git
   cd kubectl-tail
   ```

2. Build the project:
   ```bash
   cargo build --release
   ```

3. Install the plugin:
   ```bash
   cp target/release/kubectl-tail /usr/local/bin/
   ```
   Or add it to your PATH.

4. Make sure the binary is executable:
   ```bash
   chmod +x /usr/local/bin/kubectl-tail
   ```

## Usage

```bash
kubectl-tail [OPTIONS] [RESOURCES]...
```

### Options

- `-n, --namespace <NAMESPACE>`: Specify the default namespace (default: default)
- `-l, --selector <SELECTOR>`: Label selector for pods
- `-c, --container <CONTAINER>`: Container name to tail (if multi-container pod)
- `--context <CONTEXT>`: Kubernetes context to use (single value - for multi-cluster use resource format)
- `--tail <TAIL>`: Number of lines to show from the end of the logs on startup (-1 for all, 0 for none, default: follow from end)
- `-v, --verbose`: Enable verbose output for retry messages and pod events
- `-g, --grep <GREP>`: Filter logs by regex pattern (stdout mode only)
- `--no-tui`: Disable TUI mode and use stdout output (automatically enabled when piping/redirecting)
- `--buffer-size <SIZE>`: Maximum buffer size for log messages in TUI mode (default: 10000)

### Resource Format

Resources can be specified in multiple formats for flexible multi-namespace and multi-cluster tailing:

- `name` - Pod name (uses default namespace)
- `kind/name` - Resource type and name (e.g., `deployment/web`)
- `namespace/kind/name` - Namespace, resource type, and name (e.g., `production/deployment/api`)
- `context/namespace/kind/name` - Full path across clusters (e.g., `prod-us/default/deployment/web`)

**Rules:**
- If `--context` or `--namespace` flags are used, resource specs cannot override them
- If no flags are specified, you can use any format including full 4-part paths
- Contexts must exist in your kubeconfig (validated at startup)
- Resources/namespaces that don't exist at startup won't cause failure - the tool will wait for them to appear

**Note:**
- TUI mode is automatically disabled when stdout is not a terminal (e.g., when piping to another command or redirecting to a file). You don't need to specify `--no-tui` in these cases.
- In TUI mode, debug logs are written to `/tmp/kubectl-tail.log` to avoid corrupting the display. Use `tail -f /tmp/kubectl-tail.log` to monitor logs while using the TUI.

### Examples

Tail logs from pods with a specific label:
```bash
kubectl-tail -l app=nginx
```

Tail logs from a deployment:
```bash
kubectl-tail deployment/my-deployment
```

Tail logs from multiple deployments:
```bash
kubectl-tail deployment/app1 deployment/app2
```

Tail logs from a specific pod:
```bash
kubectl-tail my-pod
```

Tail logs from a specific container in a pod:
```bash
kubectl-tail my-pod -c app
```

Tail logs in a specific namespace:
```bash
kubectl-tail -n production deployment/my-deployment
```

Tail logs from multiple namespaces:
```bash
kubectl-tail default/deployment/web staging/deployment/api production/deployment/workers
```

Multi-cluster tailing (full path format):
```bash
kubectl-tail prod-us/default/deployment/web prod-eu/default/deployment/api
```

Mix namespaces with default:
```bash
kubectl-tail -n production deployment/api staging/deployment/canary
```

Pipe to grep (TUI automatically disabled):
```bash
kubectl-tail deployment/my-app | grep ERROR
```

Force stdout mode even when running in a terminal:
```bash
kubectl-tail --no-tui deployment/my-app
```

### TUI Mode Keyboard Shortcuts

When running in TUI mode (default), the following keyboard shortcuts are available:

**General:**
- `q` / `Q` / `Ctrl-C` - Quit the application
- `?` - Toggle help overlay (hint shown in status bar)
- `s` - Toggle sidebar visibility (off by default)
- `p` - Pause/Resume log streaming
- `c` - Clear log buffer
- `a` - Toggle auto-scroll (automatically scroll to bottom)
- `t` - Toggle timestamps
- `x` - Toggle pod/container prefix display

**Navigation:**
- `↑` / `↓` - Navigate sidebar (when open) or scroll logs
- `PgUp` / `PgDn` - Page up/down in logs
- `Home` / `End` or `g` / `G` - Jump to top/bottom of logs (vim-style)
- `Space` - Toggle pod/container on/off or expand/collapse tree node (in sidebar)

**Search & Filter:**
- `/` - Start search (highlights matches in yellow, press Enter to apply)
- `n` / `N` - Jump to next/previous search match
- `f` - Filter buffer (show only matching lines, press Enter to apply)
- `Esc` - Cancel search/filter input

**Note:** Search works on the currently filtered view. You can filter first (e.g., show only ERROR lines), then search within those filtered results.

## How it works

kubectl-tail collects label selectors from the specified resources and continuously watches for pods across multiple namespaces and clusters. It spawns log tailing tasks for matching running pods and automatically starts tailing new pods that match the selectors (e.g., from scaling deployments).

**Resilient Startup:** The tool won't fail if resources or namespaces don't exist at startup - it will continuously watch and automatically start tailing when they appear. Only invalid contexts will cause startup failures.

**TUI Mode (default):**
- Displays logs in a clean, minimalist terminal interface
- Borderless full-screen log view for maximum content visibility
- Optional sidebar (press `s`) with vertical line divider showing discovered pods/containers
- Interactive checkboxes to toggle pod/container log visibility
- Real-time log streaming with color-coded cluster/pod/container prefixes
- Memory-bounded ring buffer to prevent unbounded growth (default: 10,000 lines)
- Status bar shows live statistics, active filters, and help hint
- Interactive search with multiple modes and keyboard-driven navigation

**Multi-cluster & Multi-namespace Support:**
Using the resource path format (`context/namespace/kind/name`), kubectl-tail can connect to multiple clusters and namespaces simultaneously, streaming logs with cluster/namespace/pod/container-prefixed identifiers. This is useful for monitoring distributed applications across multiple Kubernetes clusters and environments in a single view.

Pods are color-coded for easy identification, and tailing stops gracefully when pods are deleted or become non-running.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the MIT License.