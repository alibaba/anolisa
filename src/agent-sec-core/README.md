# Agent Sec Core

[中文版](README_CN.md)

**OS-level security kernel for AI Agents.** Provides a full defense chain of system hardening, asset integrity verification, and security decision-making. Runs as a security supervision layer above all business skills, applicable to Agent OS platforms such as [ANOLISA](../../README.md) and OpenClaw.

## Background

As AI Agents gradually gain OS-level execution capabilities (file I/O, network access, process management, etc.), traditional application security boundaries no longer apply. Agent Sec Core builds a **defense-in-depth** system at the OS layer, ensuring Agents run in a controlled, auditable, least-privilege environment.

## Core Principles

1. **Least Privilege** — Agents receive only the minimum system permissions required to complete a task.
2. **Explicit Authorization** — Sensitive operations require explicit user confirmation; silent privilege escalation is forbidden.
3. **Zero Trust** — Skills are mutually untrusted; each operation is independently authenticated.
4. **Defense in Depth** — System hardening → Asset verification → Security decision. Compromise of any single layer does not affect the others.
5. **Security Over Execution** — When security and functionality conflict, security wins. When in doubt, treat as high risk.

## Security Architecture

```
┌─────────────────────────────────────────────┐
│              Agent Application              │
├──────────────────┬──────────────────────────┤
│ Security Check   │  Sandbox Policy          │
│ Workflow         │  (agent-sec-sandbox,      │
│ (SKILL.md)       │   managed independently)  │
├──────────────────┴──────────────────────────┤
│  4. Security Decision (Risk Classification) │
├─────────────────────────────────────────────┤
│  Phase 3: Final Security Confirmation       │
├─────────────────────────────────────────────┤
│  Phase 2: Asset Protection (GPG + SHA-256)  │
├─────────────────────────────────────────────┤
│  Phase 1: System Hardening (loongshield)    │
├─────────────────────────────────────────────┤
│              Linux Kernel                   │
└─────────────────────────────────────────────┘
```

The security check workflow (Phase 1-3 + Security Decision) is defined in `skill/SKILL.md`. Sandbox policy is managed independently by `skill/references/agent-sec-sandbox.md`.

## Security Check Workflow

Each Agent execution must complete the following security checks **in strict order** (Phase 1-3). Only after all phases pass can the security decision process proceed. See `skill/SKILL.md` for the full executable protocol.

| Phase | Description | Entry | PASS Condition |
|-------|-------------|-------|----------------|
| **Phase 1** | System Hardening — `loongshield seharden --scan --config agentos_baseline` | `skill/references/agent-sec-seharden.md` | Output contains `结果：合规` |
| **Phase 2** | Asset Protection — GPG signature + SHA-256 hash verification of all skills | `skill/references/agent-sec-skill-verify.md` | Output contains `VERIFICATION PASSED` |
| **Phase 3** | Final Confirmation — Re-run Phase 1 scan + Phase 2 verify as recheck | `skill/SKILL.md` | Both rechecks pass |

If any phase is not PASS, all subsequent phases are cancelled and the agent execution is blocked.

## Risk Classification

| Level | Examples | Action |
|-------|----------|--------|
| **Low** | File reads, info queries, text processing | Allow (sandboxed) |
| **Medium** | Code execution, package install, external API calls | Sandbox isolation + user confirmation |
| **High** | Reading `.env` / SSH keys, data exfiltration, modifying system config | Block unless explicitly approved |
| **Critical** | Prompt injection, secret leakage, disabling security policies | Immediate block + audit log + notify user |

**When in doubt, treat as high risk.**

## Protected Assets

### System Credentials

Agents are **never** allowed to access or exfiltrate:

- SSH keys (`/etc/ssh/`, `~/.ssh/`)
- GPG private keys
- API tokens / OAuth credentials
- Database credentials
- `/etc/shadow`, `/etc/gshadow`
- Host identity information (IP, MAC, `hostname`)

### Critical System Files

The following paths are write-protected:

- `/etc/passwd`, `/etc/shadow`, `/etc/sudoers`
- `/etc/ssh/sshd_config`, `/etc/pam.d/`, `/etc/security/`
- `/etc/sysctl.conf`, `/etc/sysctl.d/`
- `/boot/`, `/usr/lib/systemd/`, `/etc/systemd/system/`

## Sandbox Policy Templates

`linux-sandbox` provides 3 built-in policy templates:

| Template | Filesystem | Network | Use Case |
|----------|-----------|---------|----------|
| **read-only** | Entire filesystem read-only | Denied | Read-only operations: `ls`, `cat`, `grep`, `git status`, etc. |
| **workspace-write** | cwd + /tmp writable, rest read-only | Denied | Build, edit, script execution requiring file writes |
| **danger-full-access** | Unrestricted | Allowed | ⚠ Reserved template, for special scenarios only |

Command classification maps directly to sandbox modes:

| Classification | Sandbox Mode | Description |
|---------------|-------------|-------------|
| `destructive` | ❌ Rejected | Dangerous commands, execution refused |
| `dangerous` | workspace-write | High-risk operations, no extra permissions allowed |
| `safe` | read-only | Read-only operations, no extra permissions needed |
| `default` | workspace-write | Normal operations, network/write paths added as needed |

## Project Structure

```
agent-sec-core/
├── linux-sandbox/             # Rust sandbox executor (bubblewrap + seccomp)
│   ├── src/                   # Rust source (cli, policy, seccomp, proxy, …)
│   ├── tests/                 # Rust integration tests + Python e2e
│   └── docs/                  # dev-guide, user-guide
├── agent-sec-cli/             # Unified CLI + security middleware (Python)
│   ├── src/agent_sec_cli/     # Main Python package
│   │   ├── cli.py             # CLI entry point (Typer)
│   │   ├── asset_verify/      # Skill signature + hash verification
│   │   ├── code_scanner/      # Code security scanning engine
│   │   ├── sandbox/           # Sandbox policy generation
│   │   ├── skill_ledger/      # Ed25519 integrity ledger (check/certify/status)
│   │   ├── security_events/   # JSONL event logging
│   │   └── security_middleware/ # Middleware layer + backends
│   ├── dev-tools/             # Developer guides for extending backends
│   └── pyproject.toml         # Build configuration
├── skill/
│   ├── SKILL.md               # Executable security protocol (check workflow + decision)
│   └── references/
│       ├── agent-sec-seharden.md       # Phase 1 sub-skill (loongshield hardening)
│       ├── agent-sec-sandbox.md        # Sandbox policy configuration guide
│       └── agent-sec-skill-verify.md   # Phase 2 sub-skill (asset verification)
├── tools/                     # sign-skill.sh — PGP skill signing utility
├── tests/                     # Unit, integration, and e2e tests
├── LICENSE
├── Makefile
├── agent-sec-core.spec        # RPM packaging spec
├── README.md
└── README_CN.md
```

## Quick Start

### Prerequisites

| Component | Requirement |
|-----------|-------------|
| **OS** | Alibaba Cloud Linux / Anolis / RHEL family |
| **Permissions** | root or sudo |
| **loongshield** | >= 1.1.1 (Phase 1 system hardening) |
| **gpg / gnupg2** | >= 2.0 (Phase 2 asset signature verification) |
| **Python3** | >= 3.6 |
| **Rust** | >= 1.91 (for building linux-sandbox) |

### Run the Security Workflow

```bash
# ===== Phase 1: System Hardening =====
# Baseline scan
sudo loongshield seharden --scan --config agentos_baseline

# Dry-run remediation (optional)
sudo loongshield seharden --reinforce --dry-run --config agentos_baseline

# Execute auto-hardening
sudo loongshield seharden --reinforce --config agentos_baseline

# ===== Phase 2: Asset Protection =====
# Verify all skills
agent-sec-cli verify

# Verify a single skill (optional)
agent-sec-cli verify --skill /path/to/skill_name

# ===== Phase 3: Final Confirmation =====
# Re-scan to confirm compliance
sudo loongshield seharden --scan --config agentos_baseline
agent-sec-cli verify
```

### Build Sandbox from Source

```bash
make build-sandbox
```

The binary is output to `linux-sandbox/target/release/linux-sandbox`.

### Install via RPM

```bash
sudo yum install agent-sec-core
```

### Generate Sandbox Policy

Classify a command and generate a `linux-sandbox` execution policy:

```bash
python3 agent-sec-cli/src/agent_sec_cli/sandbox/sandbox_policy.py --cwd "$PWD" "git status"
```

Output example:
```json
{
  "decision": "sandbox",
  "classification": "safe",
  "sandbox_mode": "read-only",
  "sandbox_command": "linux-sandbox --sandbox-policy-cwd ... -- git status"
}
```

## Asset Integrity Verification

### Verification Flow

1. Load trusted public keys from `agent-sec-cli/asset-verify/trusted-keys/*.asc`
2. Verify the GPG signature (`.skill-meta/.skill.sig`) of `.skill-meta/Manifest.json` in each skill directory
3. Validate SHA-256 hashes of all files listed in the Manifest

### Error Codes

| Code | Meaning |
|------|---------|
| 0 | Passed |
| 10 | Missing `.skill-meta/.skill.sig` |
| 11 | Missing `.skill-meta/Manifest.json` |
| 12 | Invalid signature |
| 13 | Hash mismatch |

### Sign Skills (Self-Deployment Quick Start)

When deploying from source, skills are unsigned by default. Sign them so Phase 2 passes:

```bash
# 1. One-time: generate GPG key + export public key
tools/sign-skill.sh --init

# 2. Batch-sign all skills
tools/sign-skill.sh --batch /usr/share/anolisa/skills --force

# 3. Verify
agent-sec-cli verify
```

For the complete guide (manual key management, custom skills, CI/CD, troubleshooting), see **[Skill Signing Guide](tools/SIGNING_GUIDE.md)**.

## Skill Ledger

Ed25519-based integrity ledger for skill directories. Tracks file hashes, version chains, and scan results in `.skill-meta/` manifests — all managed via the `agent-sec-cli skill-ledger` subcommand.

### Key Commands

| Command | Description |
|---------|-------------|
| `init-keys` | Generate Ed25519 signing keypair |
| `check <dir>` | Detect drift / tampering against the manifest |
| `certify <dir>` | Run scanner, sign, and seal the manifest |
| `status` | System-wide health overview (keys, config, aggregate integrity) |
| `audit <dir>` | Show version history and signature chain |
| `check --all` / `certify --all` | Batch mode across all registered skill dirs |

### Quick Example

```bash
# Generate signing keys (one-time)
agent-sec-cli skill-ledger init-keys

# Check integrity (creates unsigned baseline on first run)
agent-sec-cli skill-ledger check /path/to/skill

# Certify after review
agent-sec-cli skill-ledger certify /path/to/skill

# System health overview
agent-sec-cli skill-ledger status
```

Design doc: [`docs/design/SKILL_LEDGER_CN.md`](docs/design/SKILL_LEDGER_CN.md) · User guide: [`docs/guide/SKILL_LEDGER_USER_GUIDE_CN.md`](docs/guide/SKILL_LEDGER_USER_GUIDE_CN.md)

## Audit Log

All security events are logged as JSONL to `/var/log/agent-sec/security-events.jsonl` (falls back to `~/.agent-sec-core/security-events.jsonl`):

```json
{"event_id": "uuid", "event_type": "harden", "category": "hardening", "timestamp": "ISO-8601", "trace_id": "uuid", "pid": 1234, "uid": 0, "details": {"request": {...}, "result": {...}}}
```

## Development

```bash
# Build sandbox
make build-sandbox

# Run Rust tests
cd linux-sandbox && cargo test

# Run e2e tests (requires sandbox installed)
python3 tests/e2e/linux-sandbox/e2e_test.py

# Format Python code
make python-code-pretty
```

## License

Apache License 2.0 — see [LICENSE](../../LICENSE) for details.
