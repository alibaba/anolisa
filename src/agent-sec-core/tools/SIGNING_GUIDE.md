# Skill Signing Guide

[中文版](SIGNING_GUIDE_CN.md)

When you build and deploy ANOLISA from source, the deployed skills are **unsigned** by default. Phase 2 of the agent-sec-core security workflow requires valid GPG signatures — skill integrity checks will fail until every skill directory contains a signed `.skill-meta/Manifest.json`.

`sign-skill.sh` (this directory) provides everything you need: prerequisite checking, GPG key generation, batch signing, and public key export.

## Prerequisites

| Tool | RHEL / Anolis / Alinux | Debian / Ubuntu | Purpose |
|------|----------------------|-----------------|---------|
| **gpg** (gnupg2) | `sudo yum install -y gnupg2` | `sudo apt-get install -y gnupg` | GPG signing & verification |
| **jq** | `sudo yum install -y jq` | `sudo apt-get install -y jq` | JSON manifest generation |
| **sha256sum** | `coreutils` (usually pre-installed) | `coreutils` (usually pre-installed) | File hash computation |

Verify prerequisites with:

```bash
tools/sign-skill.sh --check
```

## Quick Start

Three commands cover the entire workflow. Step 1 is a one-time setup; step 2 should be re-run whenever skill files change.

```bash
# 1. One-time setup — generate GPG key + export public key to trusted-keys
tools/sign-skill.sh --init

# 2. Batch-sign all deployed skills (default: ~/.copilot-shell/skills/)
tools/sign-skill.sh --batch --force

# 3. Verify
python3 skill/scripts/asset-verify/verifier.py
```

`--init` automatically generates a dedicated signing key (`ANOLISA Local Deploy Key`) and
exports the public key to `~/.copilot-shell/skills/agent-sec-core/scripts/asset-verify/trusted-keys/`.
You can override the export path with `--trusted-keys-dir <DIR>`.

## Step-by-Step (Manual Key Management)

If you prefer full control over GPG key management instead of using `--init`:

### 1. Generate a GPG Key

```bash
gpg --batch --gen-key <<EOF
Key-Type: RSA
Key-Length: 4096
Name-Real: My Signing Key
Name-Email: me@example.com
Expire-Date: 2y
%no-protection
%commit
EOF
```

Confirm the key was created:

```bash
gpg --list-secret-keys me@example.com
```

### 2. Export the Public Key

The verifier loads trusted public keys from `~/.copilot-shell/skills/agent-sec-core/scripts/asset-verify/trusted-keys/`.
`--init` exports there automatically. To re-export manually:

```bash
tools/sign-skill.sh --export-key
```

Or export to a custom directory:

```bash
tools/sign-skill.sh --export-key /custom/path/to/trusted-keys/
```

Or fully manually:

```bash
gpg --armor --export me@example.com \
    > skill/scripts/asset-verify/trusted-keys/me-example-com.asc
```

### 3. Sign Skills

Sign a single skill:

```bash
tools/sign-skill.sh /usr/share/anolisa/skills/my-skill --force
```

Batch-sign all skills under a directory:

```bash
# Uses the default directory (~/.copilot-shell/skills/)
tools/sign-skill.sh --batch --force

# Or specify a custom directory
tools/sign-skill.sh --batch /usr/share/anolisa/skills --force
```

Each signed skill directory will contain:

| File | Description |
|------|-------------|
| `.skill-meta/Manifest.json` | SHA-256 hashes of all files in the skill |
| `.skill-meta/.skill.sig` | GPG detached signature of `Manifest.json` |

### 4. Configure the Verifier

When using `--batch`, the script automatically registers the skills directory in `config.conf`. For manual setups, make sure the skills directory is listed in `skill/scripts/asset-verify/config.conf`:

```ini
skills_dir = [
    /usr/share/anolisa/skills
]
```

### 5. Verify

```bash
# Verify all configured directories
python3 skill/scripts/asset-verify/verifier.py

# Verify a single skill
python3 skill/scripts/asset-verify/verifier.py --skill /usr/share/anolisa/skills/my-skill
```

Expected output on success:

```
[OK] my-skill

==================================================
PASSED: 1
FAILED: 0
==================================================
VERIFICATION PASSED
```

## Signing Custom Skills

If you create your own skills and deploy them alongside the built-in ones:

1. Place the skill directory (containing `SKILL.md`) under the skills root, e.g., `/usr/share/anolisa/skills/my-custom-skill/`.
2. Sign it:
   ```bash
   tools/sign-skill.sh /usr/share/anolisa/skills/my-custom-skill --force
   ```
3. Ensure the skills root directory is in `config.conf` (see §4 above).
4. Verify:
   ```bash
   python3 skill/scripts/asset-verify/verifier.py --skill /usr/share/anolisa/skills/my-custom-skill
   ```

## CI/CD Signing

In CI/CD pipelines where the GPG keyring is not pre-configured, pass your private key via the `GPG_PRIVATE_KEY` environment variable. The script imports it automatically:

```bash
export GPG_PRIVATE_KEY="$(cat my-private-key.asc)"
tools/sign-skill.sh --batch /path/to/skills --force
```

If the key has a passphrase:

```bash
export GPG_PRIVATE_KEY="$(cat my-private-key.asc)"
export GPG_PASSPHRASE="my-passphrase"
tools/sign-skill.sh --batch /path/to/skills --force
```

## Re-signing After Skill Updates

Whenever skill files are modified, the existing `.skill-meta/Manifest.json` hashes become stale. Re-sign with `--force`:

```bash
tools/sign-skill.sh --batch --force
```

Then verify:

```bash
python3 skill/scripts/asset-verify/verifier.py
```

## Verification Error Codes

| Code | Meaning | Typical Cause |
|------|---------|---------------|
| 0 | Passed | — |
| 10 | Missing `.skill-meta/.skill.sig` | Skill was never signed |
| 11 | Missing `.skill-meta/Manifest.json` | Skill was never signed |
| 12 | Invalid signature | Signed with a key not in `trusted-keys/` |
| 13 | Hash mismatch | Skill files changed after signing |

## sign-skill.sh Command Reference

| Mode | Command | Description |
|------|---------|-------------|
| **Init** | `--init [--trusted-keys-dir DIR]` | Generate GPG key + export public key |
| **Check** | `--check` | Verify prerequisites (gpg, jq, sha256sum) |
| **Single** | `<skill_dir> [--force]` | Sign one skill directory |
| **Batch** | `--batch [parent_dir] [--force]` | Sign all subdirectories under parent (default: `~/.copilot-shell/skills/`). Auto-registers the directory in `config.conf`. |
| **Export** | `--export-key [DIR]` | Export public key (default: `~/.copilot-shell/skills/agent-sec-core/scripts/asset-verify/trusted-keys/`) |

Common options:

| Option | Description |
|--------|-------------|
| `--force` | Overwrite existing `.skill-meta/Manifest.json` and `.skill-meta/.skill.sig` |
| `--skill-name NAME` | Override the skill name in the manifest (default: directory name) |
| `--trusted-keys-dir DIR` | Override the public key export directory (used with `--init`) |
