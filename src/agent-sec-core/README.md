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
├─────────────────────────────────────────────┤
│     Security Decision (Risk Classification) │
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

## Security Check Workflow

Each Agent execution must complete the following security checks in order (Phase 1–3). Only after all pass can the security decision process proceed:

| Phase | Description | Entry |
|-------|-------------|-------|
| **Phase 1** | System Hardening — Run `loongshield seharden --config agentos_baseline` for baseline scanning and hardening | `skill/references/agent-sec-seharden.md` |
| **Phase 2** | Asset Protection — GPG signature verification of system-level skills (Manifest + file hash) to ensure skill integrity | `skill/references/agent-sec-skill-verify.md` |
| **Phase 3** | Final Confirmation — Aggregate Phase 1–2 results; confirm security baseline is intact before entering the security decision process | `skill/SKILL.md` |

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
│   ├── SKILL.md               # Full usage guide (security workflow + decision process)
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

### Build from Source

Initialize the bundled source once if you cloned without `--recursive`:

```bash
git submodule update --init --recursive src/agent-sec-core/third_party/loongshield
```

Then build:

```bash
make build-all
```

This always builds `linux-sandbox` and tries to build the bundled `loongshield` submodule at `third_party/loongshield`.
If the host does not satisfy `loongshield`'s own `make env-check`, the `loongshield` build is skipped and `agent-sec-core` still builds successfully.

Artifacts:
- `linux-sandbox/target/release/linux-sandbox`
- `third_party/loongshield/build/src/daemon/loongshield` when the host supports building loongshield

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
2. Verify the GPG signature (`.skill.sig`) of `Manifest.json` in each skill directory
3. Validate SHA-256 hashes of all files listed in the Manifest

### Error Codes

| Code | Meaning |
|------|---------|
| 0 | Passed |
| 10 | Missing `.skill.sig` |
| 11 | Missing `Manifest.json` |
| 12 | Invalid signature |
| 13 | Hash mismatch |

### Sign a Skill

```bash
sign-skill.sh <skill-directory>
```

## Development

```bash
# Build sandbox and try the bundled loongshield dependency
make build-all

# Try only the bundled loongshield dependency
make loongshield

# Run Rust tests
cd linux-sandbox && cargo test

# Run e2e tests (requires sandbox installed)
python3 tests/e2e/linux-sandbox/e2e_test.py

# Install the full local runtime
sudo make install

# Format Python code
make python-code-pretty
```

## License

Apache License 2.0 — see [LICENSE](../../LICENSE) for details.
