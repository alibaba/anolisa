# ANOLISA User Guide

[中文版](../zh/README.md)

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
| [cosh-ng](user-entrypoint/cosh-ng/README.md) | cosh-ng | AI-native Linux terminal with an integrated Agent runtime |
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
| [Code Scanner Hook Configuration](agent-security/agent-sec-core/code-scanner.md) | agent-sec-core | Per-agent hook modes, environment variables, and fallback behavior |
| [PII Checker](agent-security/agent-sec-core/pii-checker.md) | agent-sec-core | Personal data / credential detection and redaction |
| [Skill Ledger User Guide](agent-security/agent-sec-core/skill-ledger.md) | agent-sec-core | Skill integrity chain and signing workflow |
| [OpenClaw Deployment & Upgrade](agent-security/agent-sec-core/openclaw-deploy.md) | agent-sec-core | OpenClaw plugin deployment and upgrade guide |

### Token Saving (`token-saving/`)

| Document | Component | Description |
|----------|-----------|-------------|
| [Tokenless Quick Start](token-saving/tokenless/QUICKSTART.md) | tokenless | Install, connect an agent, run the first compression, and verify |
| [Tokenless User Manual](token-saving/tokenless/user-manual.md) | tokenless | Capability boundaries, runtime behavior, and task navigation |
| [Tokenless Framework Integration](token-saving/tokenless/framework-integration.md) | tokenless | cosh, OpenClaw, Hermes, Qoder, Claude Code, Codex, and Qwen Code |
| [Tokenless CLI Reference](token-saving/tokenless/cli-reference.md) | tokenless | Compression, environment checks, Stash, MCP, and statistics commands |
| [Measuring Tokenless Savings](token-saving/tokenless/measuring-savings.md) | tokenless | Statistics, diffs, dry runs, AgentSight, and SLS measurement |
| [Tokenless Configuration and Data Privacy](token-saving/tokenless/configuration-and-privacy.md) | tokenless | Configuration precedence, local data, and sensitive workloads |
| [Tokenless Troubleshooting](token-saving/tokenless/troubleshooting.md) | tokenless | Adapters, databases, Stash, upgrades, and uninstall |
| [Agent Memory](token-saving/agent-memory.md) | agent-memory | Persistent memory, MCP tools, search and sovereignty controls |

### Runtime (`runtime/`)

| Document | Component | Description |
|----------|-----------|-------------|
| [Blaze Sandbox Runtime](runtime/blaze.md) | blaze | Opt-in VM networking and periodic storage artifact synchronization for managed sandboxes |
| [Workspace Checkpoints](runtime/ws-ckpt.md) | ws-ckpt | Instant snapshot/rollback via btrfs COW |
| [Skill Filesystem](runtime/skillfs.md) | skillfs | FUSE virtual views with progressive disclosure |
| [SkillFS Kubernetes Sidecar](runtime/skillfs-kubernetes-sidecar.md) | skillfs | Running SkillFS as a FUSE sidecar in Kubernetes |

---

## Terminology

| Term | Meaning |
|------|---------|
| Component | A software unit implementing a specific capability (e.g. `tokenless`) |
| Adapter | A bridge package connecting a component to an Agent framework |
| system mode | Installation requiring root privileges (`sudo anolisa install`) |
| user mode | Installation into user-local paths (no sudo required) |
