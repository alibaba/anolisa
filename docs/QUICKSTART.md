# ANOLISA Quick Start

[中文版](QUICKSTART_zh.md)

ANOLISA is a server-side operating layer for AI Agent workloads. It provides Token optimization, workspace checkpoints, observability, security enforcement, persistent memory, and more — all installable via a unified CLI.

---

## Install the CLI

```bash
curl -fsSL https://agentic-os.sh | sh
```

> Alinux 4 users can also install via `sudo yum install anolisa`.

Verify:

```bash
anolisa --version
```

---

## Explore Your Environment

```bash
# Check platform capabilities
anolisa env

# List available components
anolisa list
```

---

## Install Components

Install components on demand. Most components run in user mode; `agentsight` and `agent-sec-core` require system mode (sudo).

```bash
# Token optimization
anolisa install tokenless

# Workspace checkpoints (btrfs COW)
anolisa install ws-ckpt

# Observability (requires sudo — eBPF needs root)
sudo anolisa install agentsight

# Security (requires sudo)
sudo anolisa install agent-sec-core

# Persistent memory (MCP file-based)
anolisa install agent-memory

# Skill filesystem (FUSE virtual views)
anolisa install skillfs

# OS skill library
anolisa install os-skills

# Copilot Shell (AI terminal gateway)
anolisa install cosh
```

Check health:

```bash
anolisa status
```

---

## Use Components

After installation, each component operates independently:

```bash
# Copilot Shell — AI terminal assistant
cosh

# Token optimization — compress tool schemas and command output
tokenless compress-schema -f tool.json
tokenless env-check --all

# Workspace checkpoints — instant create/rollback
ws-ckpt checkpoint -w ~/project -s v1 -m "initial"
ws-ckpt rollback -w ~/project -s v1

# Observability — trace Agent Token consumption
sudo agentsight trace
agentsight token --period week
agentsight serve   # Web Dashboard: http://localhost:7396

# Security — system hardening and skill verification
agent-sec-cli harden --scan --config agentos_baseline
agent-sec-cli skill-ledger status
```

---

## Integrate with Agent Frameworks

Bridge installed components to Agent frameworks (cosh / OpenClaw / Hermes):

```bash
anolisa adapter scan                        # Discover installed frameworks
anolisa adapter enable tokenless openclaw   # tokenless → OpenClaw
anolisa adapter enable ws-ckpt hermes       # ws-ckpt → Hermes
```

---

## Next Steps

### Global

- [Full User Guide](user-guide/en/README.md) — browse all component docs by category
- [Installation Guide](user-guide/en/installation.md) — progressive install from CLI to full stack
- [Troubleshooting](user-guide/en/troubleshooting.md) — common issues and fixes

### User Entry Points

- [anolisa CLI Reference](user-guide/en/user-entrypoint/anolisa-cli.md)
- [Copilot Shell](user-guide/en/user-entrypoint/copilot-shell.md)
- [OS Skills](user-guide/en/user-entrypoint/os-skills.md)

### Runtime & Token Saving

- [Workspace Checkpoints](user-guide/en/runtime/ws-ckpt.md)
- [Skill Filesystem](user-guide/en/runtime/skillfs.md)
- [Token Optimization](user-guide/en/token-saving/tokenless.md)
- [Agent Memory](user-guide/en/token-saving/agent-memory.md)

### Observability & Security

- [AgentSight](user-guide/en/agent-observability/agentsight.md)
- [AgentSecCore](user-guide/en/agent-security/agent-sec-core.md)
