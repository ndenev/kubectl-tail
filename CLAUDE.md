# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

kubectl-tail is a Rust-based kubectl plugin that tails logs from Kubernetes pods with automatic discovery and an interactive TUI (Terminal User Interface). The application continuously watches for new pods matching specified criteria and automatically starts/stops log tailing as pods come and go. It supports multi-cluster tailing and offers both TUI and stdout modes.

## Build and Development Commands

- **Build**: `cargo build --release`
- **Run locally**: `cargo run -- [OPTIONS] [RESOURCES]...`
- **Run tests**: `cargo test`
- **Format code**: `cargo fmt`
- **Lint**: `cargo clippy`
- **Help**: `./target/release/kubectl-tail --help`

## Core Architecture

### Module Structure

The codebase is organized into focused modules:

- **`main.rs`**: Application entry point, TUI/stdout mode branching, multi-cluster initialization, and core pod watching logic
- **`cli.rs`**: Command-line interface using clap (includes --no-tui, --context, --buffer-size)
- **`kubernetes.rs`**: Kubernetes API interactions and log streaming (cluster-aware)
- **`types.rs`**: Core data structures (LogMessage with cluster, namespace, timestamp)
- **`utils.rs`**: Utility functions for selectors and color handling (separate functions for ratatui and crossterm colors)
- **`ui/`**: TUI module for interactive terminal interface
  - **`app.rs`**: Core application state with ring buffer, pod tracking, search state, UI state
  - **`events.rs`**: Event handling (keyboard input, log messages, pod updates, tick)
  - **`renderer.rs`**: Rendering orchestration using ratatui
  - **`widgets.rs`**: Custom widgets (PodList, LogView, StatusBar, HelpOverlay)
  - **`layout.rs`**: Layout definitions (sidebar, main, status bar)
- **`lib.rs`**: Library exports for testing
- **`tests.rs`**: Test cases

### Key Components

**TUI Architecture**: Event-driven architecture using tokio::select! to handle multiple event streams:
- Keyboard events from crossterm EventStream
- Log messages from Kubernetes log streams
- Pod update events from watchers (Added/Updated/Deleted)
- Tick events for periodic stat updates (every 250ms)
- In TUI mode, tracing logs are redirected to `/tmp/kubectl-tail.log` to avoid corrupting the display
- Automatically disables TUI mode when stdout is not a TTY (piping/redirecting)

**Multi-Cluster Support**: Composite PodKey (`cluster/namespace/pod/container`) enables tracking pods across multiple clusters. Each cluster gets independent watchers and connections.

**Pod Watching System**: Uses Kubernetes runtime watcher to monitor pod events. The application spawns separate watchers per cluster and per label selector/explicit pod name.

**Log Tailing**: Each pod/container gets dedicated tasks that handle log streaming with smart reconnection logic using `sinceTime` and deduplication (100-line window) to prevent log replay on reconnections.

**Resource Discovery**: Supports multiple Kubernetes resource types (deployments, statefulsets, daemonsets, jobs, replicasets, pods) by extracting their label selectors and watching for matching pods.

**Ring Buffer**: Memory-bounded VecDeque (default 10k lines) prevents unbounded memory growth. Auto-adjusts scroll offset when dropping old lines from the front.

**Graceful Handling**: Automatically stops tailing when pods are deleted or become non-running, with detailed status reporting for pod lifecycle events.

### Important Technical Details

- Uses `kube` runtime watcher for efficient pod monitoring
- Implements abort handles for clean task cancellation
- Maintains per-pod task tracking via HashMap<PodKey, Vec<AbortHandle>>
- Uses mpsc channels for log message aggregation and event dispatch
- Implements smart reconnection with `sinceTime` to avoid log replay
- Color-codes pod output using deterministic hashing (separate color functions for ratatui and crossterm)
- TUI renders at ~60fps with minimal CPU usage via efficient event-driven updates
- Supports three search modes: Filter (show matching lines only), Highlight (yellow background), Jump (navigate with n/N)
- Sidebar auto-selects first pod when it appears
- Help overlay uses AppMode::Help to capture all key events for dismissal

### Key Dependencies

- `kube` 2.0: Kubernetes client library with runtime watcher
- `k8s-openapi` 0.26: Kubernetes API types
- `tokio` 1.0: Async runtime with task spawning and channels (full features)
- `clap` 4.5: Command-line parsing with derive macros
- `ratatui` 0.29: TUI framework for rich terminal interfaces
- `crossterm` 0.29: Terminal manipulation and styling (with event-stream feature)
- `futures` 0.3: Stream processing utilities
- `regex` 1.0: Pattern matching for search and grep
- `tracing` 0.1: Structured logging with tracing-subscriber
- `chrono` 0.4: Timestamp handling (with serde feature)

## Testing

The project includes unit tests in `src/tests.rs`. Run with `cargo test`.