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
| Architecture | x86_64 / aarch64 |

## Installation

```bash
# Recommended (system mode required — eBPF needs root)
sudo anolisa install agentsight

# Alternative (Alinux, requires YUM repo configuration)
sudo yum install agentsight

# Source build (developers only)
cd src/agentsight && make build
```

## Quick Start

```bash
# Terminal 1: Start eBPF tracing (requires root)
sudo agentsight trace

# Terminal 2: Start Dashboard
agentsight serve
# Open http://localhost:7396 in browser
```

## Usage

### agentsight trace — Start eBPF Tracing

Starts kernel-level capture of AI Agent activity.

```bash
sudo agentsight trace
```

> Requires root privileges. Captures SSL/TLS traffic, process events, and file operations.

### agentsight serve — Start API & Dashboard

```bash
# Default: bind to 127.0.0.1:7396
agentsight serve

# Bind to all interfaces (remote access)
agentsight serve --host 0.0.0.0 --port 7396
```

> Ensure your firewall allows access to port 7396 if accessing remotely.

### agentsight token — Query Token Usage

```bash
# Today's usage
agentsight token

# Weekly comparison
agentsight token --period week --compare


# JSON output
agentsight token --json
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
