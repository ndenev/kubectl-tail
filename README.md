# kubectl-tail

A kubectl plugin for tailing logs from Kubernetes pods with continuous discovery.

## Features

- Tail logs from pods with automatic discovery of new pods
- Support for multiple resources (e.g., several deployments) and various resource types (pods, deployments, statefulsets, daemonsets, jobs, etc.)
- Filter by namespace, labels, and resource names
- Colorized output for easy log differentiation
- Continuous monitoring with graceful handling of pod restarts and deletions
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

- `-n, --namespace <NAMESPACE>`: Specify the namespace (default: default)
- `-l, --selector <SELECTOR>`: Label selector for pods
- `-c, --container <CONTAINER>`: Container name to tail (if multi-container pod)
- `--context <CONTEXT>`: Kubernetes context to use
- `--tail <TAIL>`: Number of lines to show from the end of the logs on startup (-1 for all, 0 for none, default: follow from end)
- `-v, --verbose`: Enable verbose output for retry messages and pod events

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

## How it works

kubectl-tail collects label selectors from the specified resources and continuously watches for pods in the namespace. It spawns log tailing tasks for matching running pods and automatically starts tailing new pods that match the selectors (e.g., from scaling deployments). Pods are color-coded for easy identification, and tailing stops gracefully when pods are deleted or become non-running.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the MIT License.