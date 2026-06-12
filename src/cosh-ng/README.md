# cosh-ng — Computable Operating System Harness

## What is cosh

**Computable Operating System Harness** — a deterministic Agent-OS interface with a single `cosh` entry point that provides dual-mode behavior:

- **Interactive mode**: Run `cosh` with no arguments to launch the TUI (equivalent to `cosh-tui`)
- **CLI mode**: Run `cosh <subsystem> <action>` for structured JSON output consumed by Agents and scripts

One command (`cosh pkg install nginx`) works across dnf/apt/zypper and returns structured JSON — no text parsing, no distro guessing.

**Status**: MVP v2 — compiles and runs, core subcommands implemented. 20 tests passing (7 unit + 13 integration).

## When to use cosh

| Scenario | Use |
|----------|-----|
| Agent needs reversible operations (checkpoint) | **cosh** |
| Agent needs cross-distro command execution (pkg/svc) | **cosh** |
| Agent needs structured JSON from system commands | **cosh** |
| Agent needs to learn operational knowledge | **OS documentation / runbooks** |
| cosh is unavailable on target host | **OS documentation / runbooks** |
| One-off commands where structured output isn't needed | **raw bash** |

## Architecture

3-crate workspace with strict dependency direction:

```
cosh-types          cosh-platform          cosh-cli
  (types only)    ← (distro detect +    ← (CLI entry point,
   zero side       backend routing)       binary: cosh)
   effects)              │                     │
                         └── depends on ───────┘
                                cosh-types

Dependency: cosh-cli → cosh-platform → cosh-types
```

### Crate layout

```
cosh-ng/
├── crates/
│   ├── cosh-types/       # Pure types, zero side effects
│   │   └── src/          # checkpoint.rs, config.rs, error.rs, output.rs, pkg.rs, svc.rs
│   ├── cosh-platform/    # Distro detection + backend routing
│   │   └── src/          # checkpoint.rs, detect.rs, pkg.rs, svc.rs
│   └── cosh-cli/         # CLI entry (binary: cosh)
│       ├── src/          # main.rs, cmd/{pkg,svc,checkpoint,audit}.rs
│       └── tests/        # 13 CLI integration tests
└── Cargo.toml
```

## Ecosystem

| Component | Relationship |
|-----------|-------------|
| **Tokenless** | Complementary — cosh generates JSON, Tokenless compresses it |
| **ws-ckpt** | cosh wraps ws-ckpt daemon capabilities via Unix socket IPC |

```
Agent Framework
  │
  │  cosh pkg install nginx
  ▼
cosh-cli
  ├── pkg/svc → cosh-platform → dnf / apt-get / systemctl
  └── checkpoint → cosh-platform → ws-ckpt daemon → btrfs snapshot (μs)
```

## Quick start

```bash
# Build
cargo build --workspace

# Interactive mode — launches TUI
cosh

# CLI mode — structured JSON output
cosh pkg install nginx
# → {"ok":true,"data":{"package":"nginx","version":"1.24.0","already_installed":false},...}

cosh pkg install nginx --dry-run   # preview without executing

# Service management (systemd)
cosh svc status nginx
# → {"ok":true,"data":{"name":"nginx","active":true,"enabled":true,"recent_logs":[...]},...}

cosh svc restart nginx --dry-run

# Workspace checkpoint (requires ws-ckpt daemon)
cosh checkpoint create --workspace /home/agent/project -m "before refactor"
# → {"ok":true,"data":{"checkpoint_id":"step-042","step":42},...}

cosh checkpoint restore step-040 --workspace /home/agent/project

# Security audit
cosh audit check --action "rm -rf /var/log"
# → {"ok":true,"data":{"action":"rm -rf /var/log","allowed":true},...}
```

## Command reference

| Subcommand | Example | Backend |
|-----------|---------|---------|
| `cosh pkg install <name>` | `cosh pkg install nginx` | dnf / apt-get / zypper |
| `cosh pkg remove <name>` | `cosh pkg remove nginx` | dnf / apt-get / zypper |
| `cosh pkg search <query>` | `cosh pkg search "web server"` | dnf / apt-cache / zypper |
| `cosh svc status <name>` | `cosh svc status nginx` | systemctl show |
| `cosh svc start/stop/restart` | `cosh svc restart nginx` | systemctl |
| `cosh svc enable/disable` | `cosh svc enable nginx` | systemctl |
| `cosh svc list` | `cosh svc list --state running` | systemctl list-units |
| `cosh checkpoint create` | `cosh checkpoint create -w /path -m "msg"` | ws-ckpt daemon |
| `cosh checkpoint list` | `cosh checkpoint list -w /path` | ws-ckpt daemon |
| `cosh checkpoint restore <id>` | `cosh checkpoint restore step-003 -w /path` | ws-ckpt daemon |
| `cosh checkpoint status` | `cosh checkpoint status -w /path` | ws-ckpt daemon |
| `cosh audit check` | `cosh audit check --action "..."` | Security subsystem (stub) |
| `cosh audit log` | `cosh audit log --session abc123` | Security subsystem (stub) |

## Output format

All commands output a unified JSON envelope (`CoshResponse<T>`):

```json
{"ok":true,"data":{...},"meta":{"subsystem":"pkg","duration_ms":342,"distro":"alinux","dry_run":false}}
```

On error:

```json
{"ok":false,"error":{"code":"PkgNotFound","message":"package 'nginx-extra' not found","recoverable":true,"hint":"try 'cosh pkg search nginx'","subsystem":"pkg"},"meta":{...}}
```

Key fields for Agents: `ok` (success?), `error.recoverable` (retry-worthy?), `error.hint` (next step suggestion).

## Agent value

1. **Zero learning** — Agent doesn't need to know dnf vs apt
2. **Structured output** — JSON, no regex text parsing
3. **Reversible** — checkpoint → execute → rollback on failure
4. **Classified errors** — `recoverable` tells Agent whether to retry
5. **Dry-run** — `--dry-run` on all write operations, preview before execute

## Supported distros

| Distro | Package manager | Service manager |
|--------|----------------|-----------------|
| Alinux 2/3 | dnf | systemd |
| CentOS 7/8/9 | dnf | systemd |
| Fedora | dnf | systemd |
| Ubuntu | apt-get | systemd |
| Debian | apt-get | systemd |
| openSUSE | zypper | systemd |

## Build and test

```bash
cargo build --workspace
cargo test --workspace
cargo test --package cosh-cli --test cli_integration  # integration only
```

**Prerequisites**: Linux, Rust 1.70+, root/sudo for pkg/svc commands, ws-ckpt daemon for checkpoint commands.

## Development Phases

| Phase | Stage | Form | Status |
|-------|-------|------|--------|
| 1 | NLP human interaction | copilot-shell (TypeScript TUI) | Done |
| 1.5 | Rust TUI | cosh-tui (ratatui) | In Progress |
| 2 | Agent command wrapping | cosh CLI (Rust + JSON) | **Current** |

## License

Apache-2.0
