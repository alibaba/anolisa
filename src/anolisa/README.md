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
| `install` | Install a component from a configured raw or RPM backend |
| `uninstall` | Uninstall a component |
| `update` | Update a component, CLI itself (`self`), or all |
| `upgrade` | Apply an RPM/system-image upgrade plan (system scope) |
| `status` | Show component health |
| `doctor` | Diagnose issues and suggest fixes |
| `logs` | Query component logs |
| `restart` | Restart a component service |
| `repair` | Reconcile state after manual RPM changes |
| `adopt` | Record an existing system RPM as adopted without default removal authority |
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

Read-only discovery is broader than mutation scope: a user invocation can
list, inspect, diagnose, and attach adapters to a visible system installation.
Lifecycle mutations still target only the selected scope. Therefore
`anolisa --install-mode user install <component>` may create a separate user
installation even when the same component is already installed system-wide.

## Global Options

| Flag | Effect |
|------|--------|
| `--dry-run` | Print plan without executing |
| `--json` | Machine-readable JSON output |
| `-v, --verbose` | Increase verbosity |
| `-q, --quiet` | Suppress non-error output |
| `--no-color` | Disable colored output |

See the [full CLI guide](../../docs/user-guide/en/user-entrypoint/anolisa-cli.md)
for command forms, scope behavior, and recovery workflows.

## Architecture

Five-crate Cargo workspace:

| Crate | Responsibility |
|-------|---------------|
| `anolisa-cli` | Command parsing, dispatch, terminal UI |
| `anolisa-core` | Component resolution, adapter management, osbase install logic |
| `anolisa-env` | Environment detection (distro, arch, capabilities) |
| `anolisa-build` | Build-time codegen and asset embedding |
| `anolisa-platform` | Filesystem layout, systemd integration, IPC, privilege helpers |

Supports dual backends: **raw** (OSS tar.gz) and **RPM** (dnf repository).
The lifecycle planner separates ANOLISA-owned files from native-package
authority and records crash-recovery intent before side effects. Component
metadata is declared through `component.toml`.

## Requirements

- Linux (x86_64 / aarch64) or macOS (arm64, limited)
- Rust ≥ 1.88 (for source build)

## License

Apache License 2.0 — see [LICENSE](../../LICENSE).
