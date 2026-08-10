# ANOLISA Quick Start

[中文版](QUICKSTART_zh.md)

ANOLISA is a server-side operating layer for AI Agent workloads. It provides Token optimization, workspace checkpoints, observability, security enforcement, persistent memory, and more — all installable via a unified CLI.

---

## Install the CLI

```bash
curl -fsSL https://get.agentic-os.sh | bash
```

> Alinux 4 users can also install via `sudo yum install anolisa`.

Verify:

```bash
anolisa --version
```

AgentSecCore's raw package requires `anolisa` 0.2.17 or later. Updating the
CLI first lets it read the current component contract and choose the published
raw package correctly:

```bash
# CLI installed by get.agentic-os.sh
anolisa update self

# RPM-owned CLI
sudo anolisa update self
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

Install components on demand. The current `cosh-ng`, `agentsight`, `sec-core`,
`ws-ckpt`, and `skillfs` artifacts require system mode; the other examples
below support user mode.

```bash
# Token optimization (via anolisa CLI)
anolisa install tokenless

# Workspace checkpoints (btrfs COW)
sudo anolisa --install-mode system install ws-ckpt

# Observability (Linux system mode)
sudo anolisa --install-mode system install agentsight

# Security runtime and adapter resources (Linux x86_64 system mode)
sudo anolisa --install-mode system install sec-core

# Persistent memory (MCP file-based)
anolisa install agent-memory

# Skill filesystem (FUSE virtual views)
sudo anolisa --install-mode system install skillfs

# OS skill library
anolisa install os-skills

# Copilot Shell
anolisa install cosh

# cosh-ng (AI-native Linux terminal)
sudo anolisa --install-mode system install cosh-ng
```

Check health:

```bash
anolisa status
```

System services are placed during installation but are not started or enabled
automatically. Start AgentSight under systemd when you are ready to collect
events:

```bash
sudo systemctl enable --now agentsight.service
sudo systemctl status agentsight.service
```

The main service starts tracing and the Dashboard together and brings up
`agentsight-enforcer.service` as its dependency.

---

## Use Components

After installation, each component operates independently:

```bash
# Start the installed terminal
cosh

# Token optimization — compress tool schemas and command output
tokenless compress-schema -f tool.json
tokenless env-check --all

# Workspace checkpoints — instant create/rollback
ws-ckpt checkpoint -w ~/project -s v1 -m "initial"
ws-ckpt rollback -w ~/project -s v1

# Observability (the system service stores data as root)
sudo agentsight token --period week
# Web Dashboard: http://localhost:7396

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
anolisa adapter enable sec-core openclaw    # AgentSecCore → OpenClaw
```

---

## Next Steps

### Global

- [Full User Guide](user-guide/en/README.md) — browse all component docs by category
- [Installation Guide](user-guide/en/installation.md) — progressive install from CLI to full stack
- [Troubleshooting](user-guide/en/troubleshooting.md) — common issues and fixes

### User Entry Points

- [anolisa CLI Reference](user-guide/en/user-entrypoint/anolisa-cli.md)
- [cosh-ng AI-native Terminal](user-guide/en/user-entrypoint/cosh-ng/QUICKSTART.md)
- [Copilot Shell](user-guide/en/user-entrypoint/copilot-shell/QUICKSTART.md)
- [OS Skills](user-guide/en/user-entrypoint/os-skills.md)

### Runtime & Token Saving

- [Workspace Checkpoints](user-guide/en/runtime/ws-ckpt.md)
- [Skill Filesystem](user-guide/en/runtime/skillfs.md)
- [Token Optimization](user-guide/en/token-saving/tokenless/QUICKSTART.md)
- [Agent Memory](user-guide/en/token-saving/agent-memory.md)

### Observability & Security

- [AgentSight](user-guide/en/agent-observability/agentsight.md)
- [AgentSecCore](user-guide/en/agent-security/agent-sec-core/QUICKSTART.md)
