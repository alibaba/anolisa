# Skill Ledger User Guide

Skill Ledger is the security subsystem of agent-sec-core that maintains a version chain of file hashes, scan results, and cryptographic signatures for AI Agent Skills, helping detect tampered Skills or injected malicious content. The default quick scan runs automatically via the built-in static scanner; an optional deep scan is driven by the Agent following the `skill-vetter` protocol.

---

## Part 1: Quick Tour

### Core Concepts

| Concept | Description |
|---------|-------------|
| **Manifest** | JSON record (`.skill-meta/latest.json`) containing file hashes, scan results, and digital signatures; created and updated by `scan`, `certify`, or the `init` baseline |
| **Version chain** | Append-only ledger — each version links to its predecessor via `previousManifestSignature`, forming a tamper-evident history |
| **Status** | Per-Skill security state: `pass` ✅ · `none` 🆕 · `drifted` 🔄 · `warn` ⚠️ · `deny` 🚨 · `tampered` 🔴 |

### 1. Initialize Signing Keys

```bash
# Initialize keys and build a quick-scan baseline for Skills in covered directories
agent-sec-cli skill-ledger init
```

Key locations:

| File | Path | Permissions |
|------|------|-------------|
| Private key file | `~/.local/share/agent-sec/skill-ledger/key.enc` | 0600; unencrypted by default, encrypted with `--passphrase` |
| Public key | `~/.local/share/agent-sec/skill-ledger/key.pub` | 0644 |

To protect the private key with a passphrase:

```bash
# Interactive passphrase prompt
agent-sec-cli skill-ledger init --passphrase

# Or via environment variable (suitable for CI)
SKILL_LEDGER_PASSPHRASE="your-secret" agent-sec-cli skill-ledger init --passphrase
```

### 2. Check Skill Integrity

```bash
agent-sec-cli skill-ledger check /path/to/your-skill
```

Outputs JSON; the key field is `status`:

| Status | Meaning |
|--------|---------|
| `none` 🆕 | Never scanned — no verifiable signed manifest |
| `pass` ✅ | Files unchanged + signature valid + scan passed |
| `drifted` 🔄 | Skill files have changed (fileHashes mismatch) |
| `warn` ⚠️ | Signature valid, but the last scan has low-risk findings |
| `deny` 🚨 | Signature valid, but the last scan has high-risk findings |
| `tampered` 🔴 | Manifest signature verification failed — metadata may be forged |

### 3. Quick Scan + Signed Certification

The default certification path uses the built-in quick scanner and does not depend on an LLM. For a single Skill:

```bash
agent-sec-cli skill-ledger scan /path/to/your-skill
```

After scanning, re-check the status:

```bash
agent-sec-cli skill-ledger check /path/to/your-skill
```

For a more thorough semantic review, trigger a deep scan through the Agent. The Agent reads the built-in `skill-vetter-protocol.md` scanning protocol and reviews the target Skill file by file across four phases (origin verification → code review → permission boundary assessment → risk grading), writing results to a findings JSON file. Then pass the findings file to `certify` to complete signed certification:

```bash
agent-sec-cli skill-ledger certify /path/to/your-skill \
  --findings /tmp/skill-vetter-findings-your-skill.json \
  --scanner skill-vetter \
  --delete-findings
```

`scan` runs the built-in quick scanner and signs the result into the ledger; `certify` only imports external findings. `certify` performs, in order:

1. Verifies file consistency (automatically creates a new version if files changed)
2. Normalizes findings and merges them into the manifest's `scans[]` array
3. Aggregates `scanStatus` (`pass` / `warn` / `deny`)
4. Re-signs and writes `.skill-meta/latest.json`

Example output:

```json
{
  "versionId": "v000002",
  "scanStatus": "pass",
  "newVersion": true,
  "skillName": "your-skill"
}
```

### 4. View Overall Security Posture

```bash
# Overall skill-ledger system status (keys, config, health of all Skills)
agent-sec-cli skill-ledger status

# Include per-Skill detailed status
agent-sec-cli skill-ledger status --verbose
```

`status` outputs JSON with three sections:

| Section | Description |
|---------|-------------|
| `keys` | Signing key state (initialized, fingerprint, encrypted, number of archived keys) |
| `config` | Configuration summary (default directories, managedSkillDirs pattern count, registered scanners) |
| `skills` | Aggregate health (discovered Skill count, per-status counts, overall health label) |

`health` label meanings: `healthy` (no critical/attention statuses and not all none; may mix pass/none), `unscanned` (all none), `attention` (drifted/warn present), `critical` (deny/tampered/error present), `empty` (no registered Skills).

With `--verbose`, an additional `results` array contains detailed check results for each Skill.

### 5. Audit the Full Version Chain

Deep-verify all historical versions — hash integrity, signature validity, and version chain links:

```bash
agent-sec-cli skill-ledger audit /path/to/your-skill

# Also verify snapshot file hashes
agent-sec-cli skill-ledger audit /path/to/your-skill --verify-snapshots
```

### 6. Agent-Driven Scanning (Recommended)

The most natural way to use Skill Ledger is through natural-language requests to an AI Agent. A default "scan" performs the quick scan; the `skill-vetter` deep scan runs only when the user explicitly requests it, or confirms continuation after a quick scan:

| Request | Effect |
|---------|--------|
| "Scan /path/to/skill" | Quick-scan certification for the specified Skill |
| "Scan all skills" | Batch quick scan of all Skills configured in `config.json` |
| "Deep scan /path/to/skill" | File-by-file deep review per the `skill-vetter` protocol, then certification |
| "Check skill status" | Output the status triage table only, without scanning |

Skill workflow:

- **Phase 1** (environment preparation and status view): validates CLI and keys, resolves target Skills, outputs a triage table
- **Phase 2** (quick-scan certification): invokes the built-in `code-scanner` and `static-scanner`, then signs into the manifest
- **Phase 3** (optional deep scan): `skill-vetter` four-phase review — origin verification → code review → permission boundary assessment → risk grading — then writes to the version chain via `certify --findings`

---

## Part 2: Protecting Skills via SkillFS Activation, User Decisions, and Host Hook Policies

### Architecture Overview

Skill Ledger is recommended in combination with SkillFS: SkillFS captures Skill changes and notifies the Skill Ledger daemon to scan and refresh `.skill-meta/activation.json`/xattr. Host hooks/capabilities can still be mounted by default with `policy = "ask"`; the user is prompted when the unified exposure summary carries a `message`, and stays silent when there is no `message` or the user has already made a decision.

```
┌──────────────────────────────────────────────────┐
│                  Agent runtime                    │
│                                                   │
│  ┌──────────────┐      ┌──────────────────────┐   │
│  │  SkillFS     │      │  skill-ledger        │   │
│  │  change      │      │  SKILL.md            │   │
│  │  capture     │      │  (on-demand deep     │   │
│  │      │        │     │   scan)              │   │
│  │      ▼        │      └──────────┬───────────┘   │
│  │ daemon notify │                 │               │
│  │      │        │                 │               │
│  │      ▼        │                 │               │
│  │ activation    │                 │               │
│  │ refresh       │                 │               │
│  └──────┤────────┘                 │               │
│         ▼                         ▼               │
│  ┌──────────────────────────────────────────┐     │
│  │       agent-sec-cli skill-ledger          │     │
│  │   show / export / decide / scan / certify │     │
│  └──────────────────────────────────────────┘     │
│                      │                            │
│                      ▼                            │
│           .skill-meta/latest.json                 │
│           .skill-meta/activation.json + xattr     │
└───────────────────────────────────────────────────┘
```

- **Recommended path — SkillFS + daemon activation**: SkillFS discovers Skill file changes; the daemon refreshes the executable activation target based on the latest signed manifest, user decisions, and the activation policy. The Agent runtime reads activation metadata instead of relying on host hook pre-checks by default.
- **Compatibility path — host hook/capability policy**: OpenClaw, Hermes, and copilot-shell can call `agent-sec-cli skill-ledger show` before a Skill loads; Qoder CLI runs a read-only `agent-sec-cli skill-ledger check` on the resolved local absolute directory before the `Skill` tool executes. The default `ask` requests user confirmation; `warn` / `debug` / `block` can be configured explicitly.
- **Agent-driven scanning**: `scan` runs the built-in quick scan and signs the result; the `skill-ledger` Skill drives the full four-phase security review when the user requests a deep scan, importing results via `certify --findings`. **Triggered on demand**, initiated by user request.

### Recommended Path: SkillFS + Daemon Activation

**How it works:**

With SkillFS enabled, the runtime entry point of Skill Ledger is handled by the daemon:

1. SkillFS captures Skill directory creation, updates, deletion, or content changes.
2. SkillFS notifies the Skill Ledger daemon's `skill_ledger.skillfs_notify_change` interface.
3. The daemon refreshes `.skill-meta/activation.json` based on the signed manifest, current file state, user decisions, and the activation policy, and writes xattr on a best-effort basis.
4. If the current risky version cannot be activated directly, the activation metadata points to the previous trusted `pass` / `warn` snapshot; if no trusted fallback exists, it points to a safe pending-review stub; `target: null` is written only for user `block` decisions or fail-safe scenarios.

### Compatibility Path: Hook / Capability Policy

When the Agent loads a Skill, the OpenClaw, Hermes, and copilot-shell hooks resolve the Skill directory, run `agent-sec-cli skill-ledger show <skill_dir>`, and let the unified `policy` control the user-visible behavior. These hooks consume only the `message` in the summary:

| Policy | Behavior |
|--------|----------|
| `ask` | Default. `message == null` passes silently; `message != null` requests user confirmation or uses the host approval UI. |
| `warn` | `message == null` passes silently; `message != null` shows a warning and passes. |
| `debug` | `message != null` only writes debug diagnostics and passes. |
| `block` | `message != null` blocks directly, using the message as the reason or alert text. |

The trigger rules for `message` are decided uniformly by Skill Ledger: no prompt when the user already has an `allow` / `always_allow` / `rollback` / `block` decision; no prompt when latest is `pass` or `warn` and directly exposable; a prompt when there is no user decision and latest is `deny` / `none` / `drifted` / `tampered`, explaining whether the current active version is a fallback or a safe pending-review stub. `latestStatus=unmanaged` means the daemon cannot manage this root and cannot write `.skill-meta` or record user decisions, so it is returned as diagnostics only with `message=null`, and every hook policy including `block` passes silently.

Qoder CLI is a low-level integrity gate: the plugin registers a dedicated `PreToolUse` hook for the `Skill` tool, builds user → project directory tables from the absolute `cwd` in the event, parses the `SKILL.md` frontmatter `name` (falling back to the directory name when frontmatter is absent), and after canonical-path and root-boundary validation runs `skill-ledger check <skill_dir>`. When frontmatter exists but `name` is missing, ambiguous, or uses a YAML scalar the hook cannot safely parse, the call is not downgraded to a non-local Skill — it is handled per the current policy. `pass` passes silently; `none` / `drifted` / `warn` / `deny` / `tampered` and `error` request confirmation, log debug only, warn-then-pass, or block per the `ask` / `debug` / `warn` / `block` policy. CLI unavailability, execution failure, timeout, or unparseable output is also handled by this four-level policy rather than a fixed fail-open.

OpenClaw defaults to `enabled=true, policy="ask"`; Hermes defaults to `enabled=true, policy="ask"`; copilot-shell and Qoder CLI register the Skill Ledger PreToolUse hook by default in their manifests, with the policy controlled via `SKILL_LEDGER_HOOK_POLICY`. Apart from the Qoder CLI low-level gate above, the other compatibility hooks remain fail-open when the CLI infrastructure misbehaves, avoiding blocked Skill loads.

The copilot-shell hook currently covers three directory classes — project / user / system: `<cwd>/.copilot-shell/skills/`, `~/.copilot-shell/skills/`, `/usr/share/anolisa/skills/`. Skills from custom, extension, remote, or other paths make the hook fail open and skip the skill-ledger check; the OpenClaw plugin extracts the Skill directory from the `SKILL.md` path it reads.

For batch certification or post-install certification, complete directory resolution and certification before letting the Agent read uncertified Skill content: avoid proactively reading an uncertified Skill's `SKILL.md` or auxiliary files before batch certification; after a successful install, locate the final local directory, confirm it contains `SKILL.md`, then run quick-scan certification.

**Enabling in OpenClaw**:

```json
{
  "capabilities": {
    "skill-ledger": {
      "enabled": true,
      "policy": "ask"
    }
  }
}
```

**Enabling in Hermes**:

```toml
[capabilities.skill-ledger]
enabled = true
timeout = 5
policy = "ask"
enable_block = false
```

**Configuring copilot-shell**: the default Cosh manifest already registers the `skill-ledger` hook. The default policy is `ask`; for warning-only, debug-only, or hard denial, set `SKILL_LEDGER_HOOK_POLICY=warn` / `debug` / `block`. This environment variable should be set by a trusted host or deployment environment — not by Skills, project scripts, or untrusted shell startup logic; to prevent policy downgrades via a tampered local shell profile, it should eventually move to a trusted host configuration source.

**Configuring Qoder CLI**: after installing `qoder-plugin`, the plugin automatically registers a `PreToolUse` hook with matcher `Skill`. The default policy is `ask`; a trusted launch environment may set `SKILL_LEDGER_HOOK_POLICY=debug` / `warn` / `block` and adjust the CLI timeout via `SKILL_LEDGER_TIMEOUT` (default 5 seconds). The hook covers local Skills under `~/.qoder/skills/` and `<cwd>/.qoder/skills/`, with user-level Skills of the same name taking precedence; only when both directory tables resolve trustworthily with no match is the call treated as a built-in, plugin, or remote Skill — passed and logged at debug. The hook never runs `init` or `scan` automatically; unsigned Skills enter the policy as `none`. After review, run `agent-sec-cli skill-ledger scan <skill_dir>` explicitly.

The global Skill Ledger `activationPolicy` belongs to SkillFS/daemon activation; the hook `policy` here only controls the user-visible behavior and log level of host hooks/capabilities.

### Reviewing and Deciding on Risky Skills

When a hook or `show` indicates the current skill needs user review, start with the unified exposure summary:

```bash
agent-sec-cli skill-ledger show /path/to/skill
```

Key fields:

| Field | Meaning |
|-------|---------|
| `latestStatus` | Status of the latest skill root or the latest signed version |
| `activeVersionId` | Version currently exposed to SkillFS; `null` means no real active version |
| `target` | Target SkillFS currently reads; pending state points to `.skill-meta/versions/__pending_decision__.snapshot` |
| `userDecision` | Currently matched user decision; `null` means no decision yet |
| `message` | Information to surface to the user; hooks stay silent when `null` |

To fully review a risky version that is not exposed, export the latest snapshot, manifest, and findings:

```bash
agent-sec-cli skill-ledger export /path/to/skill --version latest --output /tmp/skill-review
```

After review, choose via the unified `decide` command:

```bash
# Allow the current specific version; not inherited by future versions
agent-sec-cli skill-ledger decide /path/to/skill --action allow --reason "reviewed manually"

# Allow current and future versions until the user changes or clears the decision
agent-sec-cli skill-ledger decide /path/to/skill --action always_allow --reason "trusted source"

# Fully hide the current skill; the block is not inherited by future new versions
agent-sec-cli skill-ledger decide /path/to/skill --action block --reason "unsafe behavior"

# Roll back to a specific version; without --version, defaults to the current real active version
agent-sec-cli skill-ledger decide /path/to/skill --action rollback --version v000001 --reason "use previous trusted version"

# Clear the user decision on the latest manifest, restoring global activation behavior
agent-sec-cli skill-ledger decide /path/to/skill --clear
```

Note: a hook's `ask` confirmation only lets the current host operation continue — it is not equivalent to a Skill Ledger `allow`. Only `decide` changes the subsequent activation target.

### Agent-Driven Deep Scan

#### Configuring Skill Directories (for batch scans)

Five built-in directories are included by default: `~/.openclaw/skills/*`, `~/.copilot-shell/skills/*`, `~/.hermes/skills/**`, `~/.qoder/skills/*`, `/usr/share/anolisa/skills/*`. Project-level Qoder directories are not relative defaults; after an explicit `scan` or `certify` on a project Skill, its absolute directory is written to `managedSkillDirs` via the auto-memoization mechanism. To add other directories, create or edit `~/.config/agent-sec/skill-ledger/config.json`:

```json
{
  "enableDefaultSkillDirs": true,
  "managedSkillDirs": [
    "/opt/custom-skills/*",
    "/opt/custom-skills/my-skill"
  ]
}
```

Default directories are enabled by default; `managedSkillDirs` holds directories dynamically managed by skill-ledger or added by the user, appended after the defaults (deduplicated automatically). Set `enableDefaultSkillDirs` to `false` for isolated runs.

- `"path/*"` — glob pattern: each subdirectory containing `SKILL.md` counts as one Skill
- `"path/to/skill"` — a single Skill directory (must also contain `SKILL.md`)

Non-existent directories are silently ignored. Additionally, running `scan` or `certify` on a Skill auto-appends unregistered directories to the config for later `--all` batch operations. `check` is a read-only status query and never writes config.

#### Scheduled Default Quick Scans

To periodically refresh default quick-scan results, put `scan --all` into cron. `scan --all` automatically skips Skills whose files are unchanged and already have complete scan results, re-scanning only new, changed, scan-result-missing, or manifest-anomalous Skills.

Without a key passphrase:

```bash
mkdir -p "$HOME/.local/state/agent-sec"
AGENT_SEC_CLI="$(command -v agent-sec-cli)"
CRON_LINE="0 3 * * * $AGENT_SEC_CLI skill-ledger scan --all >> $HOME/.local/state/agent-sec/skill-ledger-scan.log 2>&1"
(crontab -l 2>/dev/null | grep -Fv "skill-ledger scan --all"; echo "$CRON_LINE") | crontab -
```

With a passphrase-protected private key, the scheduled job needs `SKILL_LEDGER_PASSPHRASE`. The command below writes the passphrase in plaintext to the current user's crontab and the system cron spool — use it only in trusted single-user environments; safer alternatives are the default passphrase-less key, or wrapping `scan --all` with a local secret manager / permission-restricted file.

```bash
read -rsp "SKILL_LEDGER_PASSPHRASE: " SKILL_LEDGER_PASSPHRASE; echo
mkdir -p "$HOME/.local/state/agent-sec"
AGENT_SEC_CLI="$(command -v agent-sec-cli)"
CRON_LINE="0 3 * * * SKILL_LEDGER_PASSPHRASE='$SKILL_LEDGER_PASSPHRASE' $AGENT_SEC_CLI skill-ledger scan --all >> $HOME/.local/state/agent-sec/skill-ledger-scan.log 2>&1"
(crontab -l 2>/dev/null | grep -Fv "skill-ledger scan --all"; echo "$CRON_LINE") | crontab -
unset SKILL_LEDGER_PASSPHRASE
```

Inspect installed scheduled jobs:

```bash
crontab -l
```

#### Triggering Scans

Just instruct the Agent in natural language. The default scan runs Phase 1 → Phase 2; Phase 1 → Phase 3 runs when the user explicitly requests a deep scan.

**Deep-scan rule table (skill-vetter):**

| Level | Rule ID | Detection Target |
|-------|---------|------------------|
| deny | `dangerous-exec` | Dangerous process execution (`child_process`, `subprocess`) |
| deny | `dynamic-code-eval` | Dynamic code execution (`eval()`, `new Function()`) |
| deny | `env-harvesting` | Bulk environment variable harvesting + network exfiltration |
| deny | `crypto-mining` | Mining signatures (`stratum`, `xmrig`, etc.) |
| deny | `credential-access` | Credential and sensitive file access (`~/.ssh/`, `.env`) |
| deny | `system-modification` | System file tampering (`/etc/`, crontab) |
| deny | `prompt-override` | Prompt-override instructions |
| deny | `hidden-instruction` | Hidden instructions (zero-width characters, HTML comments) |
| warn | `obfuscated-code` | Code obfuscation (very long lines, base64 + decode) |
| warn | `suspicious-network` | Suspicious network connections (direct IPs, non-standard ports) |
| warn | `exfiltration-pattern` | Data exfiltration patterns (file read + network send combos) |
| warn | `agent-data-access` | Agent identity data access (`MEMORY.md`, etc.) |
| warn | `unauthorized-install` | Undeclared package installation |
| warn | `unrestricted-tool-use` | Unconstrained tool-use instructions |
| warn | `external-fetch-exec` | External fetch-and-execute (`curl | bash`) |
| warn | `privilege-escalation` | Privilege escalation (`sudo`, `chmod 777`) |

### Real-World Scenarios

#### Scenario A: Detecting tampering when loading a third-party Skill

```
# SkillFS/daemon or host hook detects an anomalous status
[skill-ledger] 🚨 Skill 'third-party-tool' metadata signature verification failed
```

The alert indicates someone may have modified the manifest, flipping `scanStatus` from `deny` to `pass` to bypass security checks.

#### Scenario B: Detecting drift after a Skill update

```bash
agent-sec-cli skill-ledger check /path/to/my-skill
# → {"status": "drifted", "added": [...], "modified": [...]}
```

The status becomes `drifted` after updating the Skill. Trigger a re-scan to restore `pass`:

```
Scan /path/to/my-skill
```

#### Scenario C: Auditing historical integrity

```bash
agent-sec-cli skill-ledger audit /path/to/my-skill --verify-snapshots
```

Per-version verification: hash integrity → signature validity → version chain links → snapshot consistency.

---

## Command Cheat Sheet

| Command | Purpose |
|---------|---------|
| `agent-sec-cli skill-ledger init` | Initialize keys and build a quick-scan baseline for covered Skills |
| `agent-sec-cli skill-ledger init --no-baseline` | Initialize keys only, without scanning Skills |
| `agent-sec-cli skill-ledger check <dir>` | Check integrity status (JSON output) |
| `agent-sec-cli skill-ledger show <dir>` | Show latest, active, user decision, activation target, findings, and alerts |
| `agent-sec-cli skill-ledger export <dir> --version latest --output <path>` | Export a snapshot, manifest, and findings for full review |
| `agent-sec-cli skill-ledger decide <dir> --action allow|always_allow|block|rollback` | Record a user decision and refresh activation |
| `agent-sec-cli skill-ledger decide <dir> --clear` | Clear the user decision on the latest manifest |
| `agent-sec-cli skill-ledger scan <dir>` | Run a quick scan and sign it into the manifest |
| `agent-sec-cli skill-ledger scan --all` | Gap-filling quick scan across all discovered Skills |
| `agent-sec-cli skill-ledger certify <dir> --findings <file>` | Sign deep-scan findings into the manifest |
| `agent-sec-cli skill-ledger status` | Overall security posture (keys, config, Skill health) |
| `agent-sec-cli skill-ledger status --verbose` | Overall posture including per-Skill detailed results |
| `agent-sec-cli skill-ledger audit <dir>` | Deep-verify the version chain |
| `agent-sec-cli skill-ledger list-scanners` | List registered scanners |

## Key Paths

| Path | Purpose |
|------|---------|
| `~/.local/share/agent-sec/skill-ledger/key.enc` | Private key file (unencrypted by default, encrypted with `--passphrase`) |
| `~/.local/share/agent-sec/skill-ledger/key.pub` | Public key |
| `~/.local/share/agent-sec/skill-ledger/keyring/` | Archived historical public keys (after key rotation) |
| `~/.config/agent-sec/skill-ledger/config.json` | Configuration file (managedSkillDirs, scanners) |
| `<skill_dir>/.skill-meta/latest.json` | Current manifest (written by `scan`, `certify`, or the `init` baseline) |
| `<skill_dir>/.skill-meta/versions/` | Version chain history |
