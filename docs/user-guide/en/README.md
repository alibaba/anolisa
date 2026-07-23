# ANOLISA User Guide

ANOLISA provides a complete server-side runtime for AI Agent workloads. Components are installed via the `anolisa` CLI and operate independently.

---

## Component Architecture

```
┌────────────────────────────────────────────────────────────────────┐
│  Agent Applications (cosh / OpenClaw / Hermes / custom)            │
├────────────────────────────────────────────────────────────────────┤
│  User Entry Points                                                 │
│  anolisa-cli · cosh · os-skills                                    │
├──────────────────────────────────┬─────────────────────────────────┤
│  Token Saving                    │  Runtime                        │
│  tokenless · agent-memory        │  skillfs · ws-ckpt              │
├──────────────────────────────────┼─────────────────────────────────┤
│  Agent Observability             │  Agent Security                 │
│  agentsight                      │  agent-sec-core                 │
└──────────────────────────────────┴─────────────────────────────────┘
```

---

## Documentation Index

### Global

| Document | Content |
|----------|---------|
| [Installation](installation.md) | Progressive install from CLI to full component stack |
| [Troubleshooting](troubleshooting.md) | Cross-component common issues and fixes |

### User Entry Points (`user-entrypoint/`)

| Document | Component | Description |
|----------|-----------|-------------|
| [anolisa CLI](user-entrypoint/anolisa-cli.md) | anolisa | Unified CLI for component management |
| [Copilot Shell](user-entrypoint/copilot-shell/QUICKSTART.md) | cosh | AI terminal assistant and command gateway |
| [OS Skills](user-entrypoint/os-skills.md) | os-skills | System management and DevOps skills |

### Agent Observability (`agent-observability/`)

| Document | Component | Description |
|----------|-----------|-------------|
| [AgentSight](agent-observability/agentsight.md) | agentsight | eBPF-based tracing, Token accounting, Web Dashboard |

### Agent Security (`agent-security/`)

| Document | Component | Description |
|----------|-----------|-------------|
| [AgentSecCore](agent-security/agent-sec-core/QUICKSTART.md) | agent-sec-core | Hardening, code scanning, prompt scanning, skill ledger |
| [PII Checker](agent-security/agent-sec-core/pii-checker.md) | agent-sec-core | Personal data / credential detection and redaction |
| [Skill Ledger User Guide](agent-security/agent-sec-core/skill-ledger.md) | agent-sec-core | Skill integrity chain and signing workflow |
| [OpenClaw Deployment & Upgrade](agent-security/agent-sec-core/openclaw-deploy.md) | agent-sec-core | OpenClaw plugin deployment and upgrade guide |

### Token Saving (`token-saving/`)

| Document | Component | Description |
|----------|-----------|-------------|
| [Tokenless](token-saving/tokenless/QUICKSTART.md) | tokenless | Schema/response compression, command rewriting |
| [Tokenless User Manual](token-saving/tokenless/user-manual.md) | tokenless | Per-strategy triggers, thresholds, stats and A/B testing |
| [Agent Memory](token-saving/agent-memory/QUICKSTART.md) | agent-memory | MCP-based persistent file memory |
| [Agent Memory User Manual](token-saving/agent-memory/user-manual.md) | agent-memory | Full MCP tool reference, search, sovereignty controls |

### Runtime (`runtime/`)

| Document | Component | Description |
|----------|-----------|-------------|
| [Workspace Checkpoints](runtime/ws-ckpt.md) | ws-ckpt | Instant snapshot/rollback via btrfs COW |
| [Skill Filesystem](runtime/skillfs.md) | skillfs | FUSE virtual views with progressive disclosure |

---

## Terminology

| Term | Meaning |
|------|---------|
| Component | A software unit implementing a specific capability (e.g. `tokenless`) |
| Adapter | A bridge package connecting a component to an Agent framework |
| system mode | Installation requiring root privileges (`sudo anolisa install`) |
| user mode | Installation into user-local paths (no sudo required) |
