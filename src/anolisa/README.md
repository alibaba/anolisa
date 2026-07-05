# anolisa

[中文版](README_zh.md)

Unified CLI gateway for ANOLISA — manages component lifecycle, framework adapters, OS base layer, and system services. anolisa is the primary user entry point of the [ANOLISA](../../README.md) project, providing a single command to install, update, diagnose, and orchestrate all components.

## Quick Start

```bash
# List available components
anolisa list

# Install a component
anolisa install agent-memory

# Check component health
anolisa status agent-memory

# Update everything
anolisa update all
```

## Command Overview

### Tier 1 — Component Lifecycle

| Command | Description |
|---------|-------------|
| `list` | List available components (alias: `ls`) |
| `install` | Install a component from configured backend (raw / rpm) |
| `uninstall` | Uninstall a component |
| `update` | Update a component, CLI itself (`self`), or all |
| `status` | Show component health |
| `doctor` | Diagnose issues and suggest fixes |
| `logs` | Query component logs |
| `restart` | Restart a component service |
| `repair` | Reconcile state after manual RPM changes |
| `adopt` | Record an existing system RPM as managed |
| `forget` | Drop state record without package operations |

### Tier 2 — Management

| Command | Description |
|---------|-------------|
| `adapter` | Manage component-to-framework adapters (scan / enable / disable / status) |
| `osbase kernel` | Kernel modules and eBPF management |
| `osbase sandbox` | Sandbox runtime management (runc, gvisor, firecracker, etc.) |
| `osbase security` | Security overlay management (loongshield, seccomp-profiles) |
| `system` | System helper daemon lifecycle (setup / serve / teardown / status) |
| `register` | Join / leave Agentic OS Co-Build Program |
| `env` | Show environment detection results |
| `bug` | Generate a bug report |

## Install Modes

| Mode | Prefix | When |
|------|--------|------|
| `system` | `/usr/local` (or custom `--prefix`) | Running as root (default) |
| `user` | `~/.local` | Running as non-root (default) |

Override with `--install-mode user|system`.

## Global Options

| Flag | Effect |
|------|--------|
| `--dry-run` | Print plan without executing |
| `--json` | Machine-readable JSON output |
| `-v, --verbose` | Increase verbosity |
| `-q, --quiet` | Suppress non-error output |
| `--no-color` | Disable colored output |

## Architecture

Five-crate Cargo workspace:

| Crate | Responsibility |
|-------|---------------|
| `anolisa-cli` | Command parsing, dispatch, terminal UI |
| `anolisa-core` | Component resolution, adapter management, osbase install logic |
| `anolisa-env` | Environment detection (distro, arch, capabilities) |
| `anolisa-build` | Build-time codegen and asset embedding |
| `anolisa-platform` | Filesystem layout, systemd integration, IPC, privilege helpers |

Supports dual backends: **raw** (OSS tar.gz) and **rpm** (dnf repository). Component metadata declared via `component.toml`.

## Requirements

- Linux (x86_64 / aarch64) or macOS (arm64, limited)
- Rust ≥ 1.88 (for source build)

## License

Apache License 2.0 — see [LICENSE](../../LICENSE).
