# cosh-ng

[中文版](README_zh.md)

## What is cosh

**Computable Operating System Harness** — a deterministic Agent-OS interface that provides cross-distro structured system operations for AI Agents.

## Architecture

5-crate workspace with strict dependency direction:

```
cosh-types          cosh-platform          cosh-cli / cosh-core / cosh-shell
  (types only)    ← (distro detect +    ← (CLI entry, interactive TUI,
   zero side       backend routing)       AI-augmented shell)
   effects)

Dependency: cosh-cli / cosh-core / cosh-shell → cosh-platform → cosh-types
```

### Crate layout

```
cosh-ng/
├── crates/
│   ├── cosh-types/       # Pure types, zero side effects
│   │   └── src/          # checkpoint.rs, config.rs, error.rs, output.rs, pkg.rs, svc.rs
│   ├── cosh-platform/    # Distro detection + backend routing
│   │   └── src/          # checkpoint.rs, detect.rs, pkg.rs, svc.rs
│   ├── cosh-cli/         # CLI entry (binary: cosh-cli)
│   │   ├── src/          # main.rs, cmd/{pkg,svc,checkpoint,audit}.rs
│   │   └── tests/        # CLI integration tests
│   ├── cosh-core/        # Interactive TUI + headless JSONL backend (binary: cosh-core)
│   │   └── src/          # LLM chat, tool execution, hook system, session management
│   └── cosh-shell/       # AI-augmented interactive shell (binary: cosh-shell)
│       ├── src/          # PTY host, OSC markers, approval control, streaming AI
│       └── tests/        # Protocol + integration tests
└── Cargo.toml
```

## Quick start

```bash
# Build
cargo build --workspace

# Structured JSON output
cosh-cli pkg install nginx
# → {"ok":true,"data":{"package":"nginx","version":"1.24.0","already_installed":false},...}

cosh-cli pkg install nginx --dry-run   # preview without executing

# Service management (systemd)
cosh-cli svc status nginx
# → {"ok":true,"data":{"name":"nginx","active":true,"enabled":true,"recent_logs":[...]},...}

cosh-cli svc restart nginx --dry-run

# Workspace checkpoint (requires ws-ckpt daemon)
cosh-cli checkpoint create --workspace /home/agent/project --id step-042 -m "before refactor"
# → {"ok":true,"data":{"checkpoint_id":"step-042","step":42},...}

cosh-cli checkpoint restore step-040 --workspace /home/agent/project

# Security audit
cosh-cli audit check --action "rm -rf /var/log"
# → {"ok":true,"data":{"outcome":"Deny","matched_rule":"shell-deny-destructive",...},...}

# Inspect and export the unified production audit timeline
cosh-cli audit status
cosh-cli audit events --since 2h --limit 100
cosh-cli audit export --since 2h --output ./audit-incident

# Resume an Agent conversation in the current workspace
cosh-shell --resume              # Open the interactive session picker
cosh-shell --resume <session-id> # Select a known provider session
```

Inside cosh-shell, use `/session` to browse sessions, `/session list` to copy
complete session IDs, and `/session status` to inspect selected and active
identities. `/session new` (or `/new`) detaches the current provider
conversation so the next Agent request starts fresh without restarting the
shell; `/session clear ...` removes old entries after confirmation.
`/session compact` summarizes any complete conversation prefix in the
background, including a single completed Agent run, while the full transcript
remains stored. Automatic compaction waits for both model-window pressure and
a safe older-run boundary. Session recovery restores model-visible
conversation context; historical terminal evidence is intentionally not
restored. Records default to
`~/.copilot-shell/cosh-core/sessions/`; change the root with
`session.persist_dir`. Project session settings and relative store paths are
resolved from the workspace cosh-shell sends to Core. See the
[session recovery guide](../../docs/user-guide/en/user-entrypoint/cosh-ng/shell/session-recovery.md)
and [session compaction guide](../../docs/user-guide/en/user-entrypoint/cosh-ng/shell/session-compaction.md).

Each Agent request defaults to 50 model turns. If the limit is reached after the
session is persisted, the interactive shell offers another approved budget in
the same provider conversation. Override the budget with `agent.max_turns` or
`COSH_MAX_TURNS`; see the
[configuration guide](../../docs/user-guide/en/user-entrypoint/cosh-ng/configuration.md#agent-turn-budget).

Core and Shell also write a redacted, versioned audit timeline under
`$XDG_STATE_HOME/cosh/audit` or `~/.local/state/cosh/audit`. The existing
SLS/metrics export is unchanged. See the
[audit operations guide](../../docs/user-guide/en/user-entrypoint/cosh-ng/cli/audit.md)
for configuration, retention, tracing, and incident export.

## Extensions in cosh-shell

Extension management is exposed only through the `/extensions` slash command
inside `cosh-shell`. The `cosh` launcher remains a thin startup wrapper, and
`cosh-cli` does not expose extension lifecycle commands.

```text
/extensions list
/extensions info example.ops
/extensions new ./example.ops --template mcp
/extensions install ./example.ops
/extensions install https://example.com/example.ops.git --ref main
/extensions link ./example.ops
/extensions update example.ops
/extensions update --all
/extensions enable example.ops
/extensions disable example.ops
/extensions settings set example.ops region cn-hangzhou --scope user
/extensions settings list example.ops
/extensions doctor
/extensions reload
```

Install, link, and capability-changing updates use a preflight operation and
explicit `/extensions consent <operation-id>`. Sensitive settings are stored
only in the operating-system secret store and are always displayed as
`[redacted]`; workspace settings require an already trusted project. Extension
context is bounded and injected after project context. Local stdio MCP tools
use the full `<extension>/mcp/<server>/<tool>` namespace and follow normal tool
approval. A hook may declare `env`, injected into that hook's own child process
only — the host process is never modified — and because that is executable
capability it is part of the extension capability fingerprint, so declaring or
changing it requires fresh consent. Agent definitions are validated and listed, but currently report
`executable=false` until the unified subagent executor is available. Registry
mutations build a candidate generation. The shell keeps one long-lived core
process: `/extensions reload` switches a healthy candidate immediately while
idle, or queues one safe-point reload while an Agent run is active. The active
run stays pinned to its original generation and the next run sees the new one.

## Command reference

| Subcommand | Example | Backend |
|-----------|---------|----------|
| `cosh-cli pkg install <name>` | `cosh-cli pkg install nginx` | dnf / apt-get / zypper |
| `cosh-cli pkg remove <name>` | `cosh-cli pkg remove nginx` | dnf / apt-get / zypper |
| `cosh-cli pkg search <query>` | `cosh-cli pkg search "web server"` | dnf / apt-cache / zypper |
| `cosh-cli svc status <name>` | `cosh-cli svc status nginx` | systemctl show |
| `cosh-cli svc start/stop/restart` | `cosh-cli svc restart nginx` | systemctl |
| `cosh-cli svc enable/disable` | `cosh-cli svc enable nginx` | systemctl |
| `cosh-cli svc list` | `cosh-cli svc list --state running` | systemctl list-units |
| `cosh-cli checkpoint create` | `cosh-cli checkpoint create --workspace /path --id snap-001 -m "msg"` | ws-ckpt daemon |
| `cosh-cli checkpoint list` | `cosh-cli checkpoint list --workspace /path` | ws-ckpt daemon |
| `cosh-cli checkpoint restore <id>` | `cosh-cli checkpoint restore step-003 --workspace /path` | ws-ckpt daemon |
| `cosh-cli checkpoint status` | `cosh-cli checkpoint status` | ws-ckpt daemon |
| `cosh-cli checkpoint init` | `cosh-cli checkpoint init --workspace /path` | ws-ckpt daemon |
| `cosh-cli checkpoint delete` | `cosh-cli checkpoint delete --snapshot snap-001` | ws-ckpt daemon |
| `cosh-cli checkpoint diff` | `cosh-cli checkpoint diff --workspace /path --from a --to b` | ws-ckpt daemon |
| `cosh-cli audit check` | `cosh-cli audit check --action "rm -rf /"` | Policy engine |
| `cosh-cli audit log` | `cosh-cli audit log --session abc123` | Policy engine |
| `cosh-cli audit status/events/trace` | `cosh-cli audit trace <id>` | Unified audit store |
| `cosh-cli audit export` | `cosh-cli audit export --since 2h --output ./incident` | Redacted incident bundle |
| `cosh-cli audit prune --dry-run` | `cosh-cli audit prune --dry-run` | Retention preview |
| `cosh-cli audit policy show` | `cosh-cli audit policy show` | Policy engine |

## Output format

All commands output a unified JSON envelope (`CoshResponse<T>`):

```json
{"ok":true,"data":{...},"meta":{"subsystem":"pkg","duration_ms":342,"distro":"alinux","dry_run":false}}
```

On error:

```json
{"ok":false,"error":{"code":"PkgNotFound","message":"package 'nginx-extra' not found","recoverable":true,"hint":"try 'cosh-cli pkg search nginx'","subsystem":"pkg"},"meta":{...}}
```

Key fields for Agents: `ok` (success?), `error.recoverable` (retry-worthy?), `error.hint` (next step suggestion).

## Agent value

1. **Zero learning** — Agent doesn't need to know dnf vs apt
2. **Structured output** — JSON, no regex text parsing
3. **Reversible** — checkpoint → execute → rollback on failure
4. **Classified errors** — `recoverable` tells Agent whether to retry
5. **Dry-run** — `--dry-run` on all write operations, preview before execute

## MCP tools

`cosh-core --headless` can connect to trusted MCP servers over stdio or
Streamable HTTP, discover their tools at startup, and expose them to the Agent as
`mcp__<server>__<tool>`. MCP servers are configured in user or system config;
their tools require approval unless the session uses `trust` mode. Use
`cosh-core mcp list`, `inspect`, `refresh`, `connect`, and `disconnect` to
manage configured servers; status output never includes credentials. See the
[MCP configuration reference](../../docs/user-guide/en/user-entrypoint/cosh-ng/configuration.md#mcp-servers).

## Logging

All binaries use structured logging via `tracing`. Logs are written to `~/.copilot-shell/logs/` with daily rotation.

### Log level control

| Method | Example | Scope |
|--------|---------|-------|
| Config file | `[ui] log_level = "debug"` (cosh-shell) | Persistent |
| Config file | `[logging] level = "info"` (cosh-core) | Persistent |
| Environment variable | `COSH_LOG=debug cosh-shell raw` | Per-invocation |
| CLI flag | `cosh-core --verbose` | Per-invocation |
| Legacy | `COSH_SHELL_DEBUG=1` (maps to debug) | Per-invocation |

Priority: `COSH_LOG` > `RUST_LOG` > `--verbose` > config file > default (`warn`)

Valid levels: `error`, `warn`, `info`, `debug`, `trace`

### Log files

```
~/.copilot-shell/logs/
├── cosh-shell.log.2026-06-26    # daily rotation
├── cosh-core.log.2026-06-26
└── ...
```

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

**Prerequisites**: Linux, Rust 1.74+, root/sudo for pkg/svc commands, ws-ckpt daemon for checkpoint commands.

## License

Apache-2.0
