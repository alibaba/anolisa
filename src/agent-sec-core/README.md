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
├── skill/
│   ├── SKILL.md               # Executable security protocol (check workflow + decision)
│   ├── scripts/
│   │   ├── sandbox/
│   │   │   ├── sandbox_policy.py     # Sandbox policy generator
│   │   │   ├── classify_command.py   # Command classifier
│   │   │   └── rules.py              # Classification rules
│   │   └── asset-verify/
│   │       ├── verifier.py         # Skill signature + hash verification
│   │       ├── errors.py           # Error code definitions
│   │       ├── config.conf         # Skills directory config
│   │       └── trusted-keys/       # Trusted public key directory
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
python3 skill/scripts/asset-verify/verifier.py

# Verify a single skill (optional)
python3 skill/scripts/asset-verify/verifier.py --skill /path/to/skill_name

# ===== Phase 3: Final Confirmation =====
# Re-scan to confirm compliance
sudo loongshield seharden --scan --config agentos_baseline
python3 skill/scripts/asset-verify/verifier.py
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
python3 skill/scripts/sandbox/sandbox_policy.py --cwd "$PWD" "git status"
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

1. Load trusted public keys from `skill/scripts/asset-verify/trusted-keys/*.asc`
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
python3 skill/scripts/asset-verify/verifier.py
```

For the complete guide (manual key management, custom skills, CI/CD, troubleshooting), see **[Skill Signing Guide](tools/SIGNING_GUIDE.md)**.

## Audit Log

All security events are logged to `/var/log/agent-sec/violations.log`:

```
[TIMESTAMP] [RISK_LEVEL] [CATEGORY]
skill: <skill_name>
action: <requested_action>
target: <target_resource>
decision: ALLOWED | BLOCKED | PENDING_CONFIRM
reason: <reason>
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
