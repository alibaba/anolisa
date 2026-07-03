# cosh-cli Overview

cosh-cli is the structured command-line tool of cosh-ng, providing AI Agents with a zero-learning-cost cross-distribution system operation interface. All commands output JSON-formatted `CoshResponse<T>` envelopes.

## Design Philosophy

1. **Zero Learning** — Agents don't need to distinguish between dnf / apt / zypper
2. **Structured Output** — Pure JSON, no regex parsing of text needed
3. **Reversible** — checkpoint create → execute → rollback on failure
4. **Classified Errors** — `recoverable` field tells Agents whether retry is worthwhile
5. **Dry-run** — All write operations support `--dry-run` preview

## Command Subsystems

| Subsystem | Description | Documentation |
|-----------|-------------|---------------|
| `pkg` | Cross-distribution package management | [package-management.md](package-management.md) |
| `svc` | systemd service management | [service-management.md](service-management.md) |
| `checkpoint` | Workspace snapshots (ws-ckpt) | [checkpoint.md](checkpoint.md) |
| `audit` | Security policy auditing | [audit.md](audit.md) |

## Common Options

```
cosh-cli <SUBCOMMAND> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--help` / `-h` | Display help information |
| `--version` / `-V` | Display version number |
| `--dry-run` | Preview mode, no actual execution (supported by each write subcommand) |

## Quick Examples

```bash
# Package management
cosh-cli pkg install nginx
cosh-cli pkg search "web server"
cosh-cli pkg list --installed
cosh-cli pkg remove nginx --dry-run

# Service management
cosh-cli svc status nginx
cosh-cli svc restart nginx --dry-run
cosh-cli svc list --state running

# Workspace snapshots
cosh-cli checkpoint create --workspace /home/agent/project --id step-042 -m "before refactor"
cosh-cli checkpoint restore step-040 --workspace /home/agent/project
cosh-cli checkpoint list --workspace /home/agent/project

# Security audit
cosh-cli audit check --action "rm -rf /var/log"
cosh-cli audit log --session abc123
cosh-cli audit policy show
```
