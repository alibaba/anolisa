# AgentSight

AgentSight is a zero-instrumentation AI Agent observability tool based on eBPF. It captures LLM API calls, Token consumption, and process behavior at the kernel level without modifying Agent code.

## Overview

AgentSight provides full-stack observability for AI Agents running on Linux:

| Capability | Description |
|------------|-------------|
| Token consumption analysis | Multi-dimensional Token accounting by agent, task, and model |
| Behavior audit | Complete tracing of LLM calls and process execution |
| Dashboard visualization | Web UI for real-time Token trends, Agent health, and session traces |
| Agent auto-discovery | Automatic detection of running AI Agent processes |
| Interruption detection | Detection of LLM errors, SSE truncation, context overflow, and crashes |
| External log export | Supports exporting structured events to external log services |

## Prerequisites

| Requirement | Minimum |
|-------------|---------|
| OS | Linux |
| Kernel | >= 5.8 (BTF support required) |
| Privileges | root or CAP_BPF (for eBPF probes) |
| ANOLISA raw package | Linux x86_64, system mode |

> **macOS**: On macOS, AgentSight provides two commands — `trace` (trajectory collector that scans local JSONL session files, no eBPF) and `serve` (Dashboard viewer). All other eBPF-dependent commands are Linux-only.

## Installation

Install the published component with the ANOLISA CLI:

```bash
# Recommended (system mode required — eBPF needs root)
sudo anolisa install agentsight

# Alternative (Alinux, requires YUM repo configuration)
sudo yum install agentsight

# Source build (developers only)
cd src/agentsight && make build-all
```

> Use `make build-all` for source builds: it builds the Dashboard frontend, the main binary, and `agentsight-enforcer` in sequence. Running only `make build` skips the enforcer, and `serve` will keep logging `AgentSight enforcement unavailable`.

## Quick Start

Use the systemd unit for a normal deployment. It runs eBPF tracing and the
Dashboard together and starts the enforcer dependency in the required order:

```bash
sudo systemctl enable --now agentsight.service
sudo systemctl status agentsight.service
```

Open `http://localhost:7396` after the service becomes active. Enabling the
main unit also keeps AgentSight available after a reboot.

The bundled systemd launcher binds the Dashboard to `0.0.0.0`. Restrict port
7396 with a firewall or security group before exposing the host to an
untrusted network.

The service runs as root with a private umask and stores data under
`/var/log/sysak/.agentsight`. Use `sudo` for CLI queries and Dashboard access
commands that read this service-owned data.

For foreground troubleshooting, stop the systemd unit first so it does not
compete with a second tracer. Then use two terminals and run both commands as
root. The second command is not reached if both are entered sequentially
because `agentsight trace` stays in the foreground:

```bash
sudo systemctl stop agentsight.service

# Terminal 1
sudo agentsight trace

# Terminal 2: Start Dashboard
sudo agentsight serve
# Open http://localhost:7396 in browser

# Print the Dashboard URL and token; open the URL as your desktop user
sudo agentsight dashboard --no-open
```

> Localhost access is authentication-free; remote access requires a token, see [Dashboard Access & Authentication](#dashboard-access--authentication).

## Usage

### agentsight trace — Start eBPF Tracing

Starts kernel-level capture of AI Agent activity.

```bash
sudo agentsight trace
```

> Requires root privileges. Captures SSL/TLS traffic, process events, and file operations.
> Run `sudo systemctl stop agentsight.service` before starting a foreground tracer.

### agentsight serve — Start API & Dashboard

```bash
# Default: bind to 127.0.0.1:7396
sudo agentsight serve

# Bind to all interfaces (remote access)
sudo agentsight serve --host 0.0.0.0 --port 7396
```

Run `serve` as the same user that runs `trace` so both commands resolve the
same data directory. Binding to `0.0.0.0` exposes the Dashboard on every
interface; restrict network access before using that form.

#### Dashboard Access & Authentication

Dashboard token authentication is enabled by default:

- **Localhost access** (loopback) bypasses authentication — just open `http://127.0.0.1:7396`.
- **Remote access** requires a token: append `?token=<TOKEN>` to the browser URL, or set the `Authorization: Bearer <TOKEN>` HTTP header.
- The token is auto-generated on the first `serve` startup (64 hex characters) and persisted to the `.dashboard_token` file next to the database (default `/var/log/sysak/.agentsight/.dashboard_token`); it is reused across restarts.
- Run `sudo agentsight dashboard --no-open` to print the service-owned access URL and token, then open the URL as your desktop user.

To disable authentication (only recommended on trusted internal networks), set in the config file:

```json
{
  "server": { "auth": { "enabled": false } }
}
```

### agentsight dashboard — Show Dashboard Access Info

Displays the Dashboard URL and auth token, then tries to open a browser. On ECS instances it also prints a security-group configuration guide.

```bash
# Show URL and token without opening a root-owned browser
sudo agentsight dashboard --no-open
```

### agentsight summary — Unified Overview

Rolls up sessions and Token usage, interruption events grouped by severity, and Tokenless savings for a recent time window — one command for the overall health picture.

```bash
# Last 24 hours (default)
agentsight summary

# Last 7 days, JSON output
agentsight summary --last 168 --json
```

> Data sources degrade independently: a missing database contributes zeros without affecting the rest of the report.

### agentsight token — Query Token Usage

```bash
# Today's usage
sudo agentsight token

# Weekly comparison
sudo agentsight token --period week --compare

# JSON output
sudo agentsight token --json
```

### agentsight audit — Query Audit Events

```bash
# Recent events
agentsight audit

# Filter by PID and type
agentsight audit --pid 12345 --type llm

# Summary statistics
agentsight audit --summary
```

### agentsight discover — Scan for Agents

```bash
# Discover running AI Agents
agentsight discover

# List known Agent types
agentsight discover --list-known
```

### agentsight interruption — Session Interruption Events

Query and manage AI Agent session interruption events.

**Interruption types:**

| Type | Description | Default Severity |
|------|-------------|-----------------|
| `llm_error` | HTTP status >= 400 or SSE body contains error | high |
| `sse_truncated` | SSE stream ended without `finish_reason=stop` | high |
| `context_overflow` | Context length exceeded | high |
| `agent_crash` | Agent process disappeared mid-session | critical |
| `token_limit` | `finish_reason=length` with output near max | medium |

```bash
# List interruption events (default: last 24h)
agentsight interruption list [--last <HOURS>] [--type <TYPE>] [--severity <LEVEL>]

# Statistics by type
agentsight interruption stats

# Count by severity
agentsight interruption count

# Get a single event by ID
agentsight interruption get <ID>

# List all interruption events of a session / conversation
agentsight interruption session <SESSION_ID>
agentsight interruption conversation <CONVERSATION_ID>

# Mark as resolved
agentsight interruption resolve <ID>
```

## Configuration

Configuration file: `/etc/agentsight/config.json` (override with `--config`).

> **Important**: User config files **replace** (not extend) the built-in default rules. Ensure your config includes all Agent rules you need.

### Feature Flags

| Feature | JSON Path | Default | Description |
|---------|-----------|---------|-------------|
| Token stats | `features.token_stats` | `true` | Core Token accounting |
| SQLite storage | `features.sqlite_storage.enabled` | `true` | Local persistence |
| Interruption detection | `features.interruption_detection.enabled` | `true` | Error/crash detection |
| Audit | `features.audit` | `true` | LLM call audit |
| Session mapping | `features.session_mapping.enabled` | `true` | responseId→sessionId |

### Runtime Limits

| Config | Default | Description |
|--------|---------|-------------|
| `event_channel_capacity` | 10,000 | Probe event bounded channel capacity |
| `pending_genai_max_count` | 1,000 | Max events awaiting session_id |
| `max_connection_body_mb` | 8 | Single HTTP connection body buffer limit |
| `ring_buffer_mb` | 32 | eBPF Ring Buffer size (must be power of 2) |

## Agent Framework Integration

### Conversational Skill (cosh)

AgentSight provides a built-in conversational skill for Copilot Shell. Users can query Token usage and audit logs via natural language:

- "How much Token did I use today?"
- "Show me today's LLM call records"

### Token Savings (Tokenless Integration)

AgentSight integrates with the Tokenless component to display Token savings data in the Dashboard. No additional configuration needed — if both are installed, savings data appears automatically.

## Data Management

### Database Auto-cleanup

Default maximum database size: 200 MB. When reached, automatic cleanup triggers.

Customize via environment variable:
```bash
export AGENTSIGHT_GENAI_DB_MAX_SIZE_MB=500
```

### Clear History

```bash
rm -rf /var/log/sysak/.agentsight
# Then restart AgentSight
```

## FAQ

**Q: Why can't I see Token data for OpenClaw?**

A: AgentSight monitors the `openclaw-gateway` daemon. Check client-gateway connectivity. If you see "pairing required" errors, run `openclaw devices approve`.

**Q: Why does the Token savings page show 0?**

A: Possible causes: (1) The AK/SK authentication mode is not yet supported; (2) Session ID format is non-standard UUID.

**Q: Why do cumulative savings exceed the single-call difference?**

A: Agents include historical messages in context. Savings accumulate across turns, so cumulative savings exceed per-turn differences.
