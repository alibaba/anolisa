# Quick Start

cosh-ng (Computable Operating System Harness) provides deterministic cross-distribution system operation interfaces for AI Agents. It consists of three binaries:

- **cosh-cli** — Structured JSON CLI covering package management, service management, workspace checkpoints, and security auditing
- **cosh-core** — Headless JSONL backend integrating LLM providers, hooks, tools, and skills
- **cosh-shell** — AI-enhanced interactive terminal with PTY host, streaming analysis, and tool approval

## Prerequisites

- Linux (Alinux / CentOS / Ubuntu / Debian / Fedora / openSUSE) or macOS (limited functionality)
- Rust 1.74+
- pkg/svc commands require root or sudo privileges
- checkpoint commands require a running ws-ckpt daemon

## Build

```bash
cd src/cosh-ng
cargo build --workspace
```

Build artifacts are located under `target/debug/`: `cosh-cli`, `cosh-core`, `cosh-shell`.

Release build:

```bash
cargo build --workspace --release
```

## First Run

### cosh-cli: Structured System Operations

```bash
# Install a package (JSON output)
cosh-cli pkg install nginx
# → {"ok":true,"data":{"package":"nginx","version":"1.24.0","already_installed":false},...}

# Preview mode (no actual execution)
cosh-cli pkg install nginx --dry-run

# Check service status
cosh-cli svc status nginx
# → {"ok":true,"data":{"name":"nginx","active":true,"enabled":true,"recent_logs":[...]},...}
```

### cosh-core: AI Agent Backend

```bash
# Single prompt execution
cosh-core --headless "Check disk usage"

# Or pipe into headless mode
echo '{"type":"user","message":{"role":"user","content":"List files in current directory"}}' | cosh-core --headless
```

### cosh-shell: Interactive Terminal

```bash
# Start interactive AI Shell
cosh-shell

# Browse resumable conversations for the current workspace
cosh-shell --resume

# Or select a known provider session directly
cosh-shell --resume <session-id>
```

## Configuration

Configuration file is located at `~/.copilot-shell/config.toml`. A default configuration is automatically created on first run.

See [Configuration](configuration.md) for details.

## Next Steps

- [cosh-cli Overview](cli/overview.md) — Learn about the CLI subsystems
- [cosh-core Overview](core/overview.md) — Learn about headless mode and LLM integration
- [cosh-shell Overview](shell/overview.md) — Learn about the interactive terminal
- [Session Recovery](shell/session-recovery.md) — Resume, inspect, and safely clear Agent conversations
- [Output Format](output-format.md) — Understand the JSON envelope and error codes
