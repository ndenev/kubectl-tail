# kubectl-tail

A kubectl plugin for tailing logs from Kubernetes pods with continuous discovery.

## Features

- Tail logs from pods with automatic discovery of new pods
- Support for various resource types (pods, deployments, statefulsets, daemonsets, jobs, etc.)
- Filter by namespace, labels, and resource names
- Continuous monitoring with graceful handling of pod restarts and deletions

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
kubectl-tail [OPTIONS] [RESOURCE]
```

### Options

- `-n, --namespace <NAMESPACE>`: Specify the namespace (default: default)
- `-l, --selector <SELECTOR>`: Label selector for pods
- `-c, --container <CONTAINER>`: Container name to tail (if multi-container pod)
- `--context <CONTEXT>`: Kubernetes context to use

### Examples

Tail logs from all pods in the default namespace:
```bash
kubectl-tail
```

Tail logs from pods with a specific label:
```bash
kubectl-tail -l app=nginx
```

Tail logs from a deployment:
```bash
kubectl-tail deployment/my-deployment
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

kubectl-tail continuously watches for pods matching the specified criteria and spawns log tailing tasks for each pod. When new pods are created or existing pods are deleted, it automatically adjusts the tailing processes accordingly.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## License

This project is licensed under the MIT License.