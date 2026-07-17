# SkillFS

SkillFS is a FUSE-based virtual filesystem for agent skills. It maps a physical
skill source tree into a stable runtime view, compiles `SKILL.md` on read, and
keeps ordinary files backed by the source tree.

SkillFS does not make business-level security decisions. External components
such as agent-sec-core or Skill Ledger scan skills and write activation state.
SkillFS consumes that state and exposes each skill as live, fallback snapshot,
or hidden.

## When to Use It

Use SkillFS when you need:

- a stable mount path for agents;
- separation between the source workspace and the agent-visible view;
- default-view filtering plus `skill-discover` for secondary skills;
- in-place policy and audit coverage for production access;
- Skill Ledger integration for fallback and hidden runtime views;
- `.skill-meta` protection from ordinary agent processes.

Do not in-place mount an existing hub workspace directly when that workspace
also contains registry metadata such as `.hub` directories or external
manifests. Keep the hub workspace and the clean SkillFS source root separate.

## Requirements

| Requirement | Details |
| --- | --- |
| OS | Linux for FUSE mounts |
| FUSE | FUSE3 (`libfuse3-dev`, `fuse3`, or equivalent) |
| Device | `/dev/fuse` must be available |
| Rust | 1.86+ for source builds |

macOS can run non-FUSE commands such as `validate`, `list`, and `classify`, but
it cannot mount SkillFS.

## Installation

```bash
# Recommended package install
anolisa install skillfs

# Source build for developers
cd src/skillfs
cargo +1.86.0 build --release
```

## Source Layout

SkillFS expects a source directory with one skill per child directory:

```text
/path/to/skills/
  demo-weather/
    SKILL.md
    scripts/
      run.sh
  demo-search/
    SKILL.md
    config.json
```

The directory name is the canonical runtime skill id. The `name` field inside
`SKILL.md` is display metadata and does not override the directory key.

Do not treat `.skill-meta` as ordinary agent data. It stores SkillFS and ledger
metadata and is hidden from ordinary callers.

## Quick Start

```bash
# Validate skills in a source directory
skillfs validate /path/to/skills

# List all skills
skillfs list /path/to/skills

# Generate skillfs-views.toml
skillfs classify /path/to/skills

# Mount the virtual filesystem
skillfs mount /path/to/skills /mnt/skillfs --foreground
```

After a normal mount, agents read:

```text
/mnt/skillfs/skills/<skill-name>/SKILL.md
```

Unmount a foreground test mount with `Ctrl+C` or:

```bash
fusermount3 -u /mnt/skillfs
```

## Mount Layouts

### Normal Mount

Normal mount uses different source and mountpoint directories:

```bash
skillfs mount /path/to/skills /mnt/skillfs --foreground
```

Agents access skills under `<MOUNTPOINT>/skills`. Direct writes to the source
directory bypass SkillFS policy and audit, while writes through the mount pass
through to the source tree.

Use normal mount for local development, compatibility checks, and environments
where the source workspace is managed by another process.

### In-place Mount

In-place mount uses the same directory for source and mountpoint:

```bash
skillfs mount /path/to/skills /path/to/skills \
  --foreground \
  --security-mode \
  --audit-log /var/log/skillfs/audit.jsonl
```

SkillFS over-mounts the source directory, so normal userspace access goes
through FUSE policy and audit. In-place mounts do not add a `/skills` layer:

```text
/path/to/skills/<skill-name>/SKILL.md
```

Use in-place mount for production security integration. Tools that replace or
rename the mountpoint directory itself, such as workspace checkpoint or rollback
tools, must run before mounting or after unmounting.

### Managed Mount

`--managed` starts a detached supervisor that keeps the mount desired state as
mounted and remounts after unexpected worker exits:

```bash
skillfs mount /path/to/skills /mnt/skillfs --managed
skillfs stop /mnt/skillfs
```

`skillfs stop <MOUNTPOINT>` clears desired state, terminates the supervisor and
worker, and unmounts. It is idempotent and safe to run when the mount is already
stopped.

Managed mode also detects stale or dead FUSE endpoints after unexpected worker
termination, clears them, and remounts with bounded recovery retries. Default
foreground mounts are unchanged: they still exit and unmount on `SIGTERM` or
`Ctrl+C`.

## CLI Utilities

### validate

```bash
skillfs validate /path/to/skills
skillfs validate /path/to/skills --format json
```

`validate` reports successful, degraded, and failed skill parses. Parse
failures are included in the status summary and produce a non-zero exit code;
degraded-only skills are reported but keep exit code 0.

In JSON output, error and warning entries include a `path` field so consumers
can locate the exact offending skill file.

### list and classify

```bash
skillfs list /path/to/skills
skillfs list /path/to/skills --enabled-only
skillfs classify /path/to/skills --primary-count 6
skillfs classify /path/to/skills --dry-run
```

`list` reports discovered skills and metadata. `classify` generates or previews
`skillfs-views.toml`; the first N skills go to the default view and the rest go
to a secondary view.

## Views and Discovery

`skillfs-views.toml` in the source directory controls visibility:

```toml
[[view]]
name = "major"
default = true
description = "Core skills shown directly in /skills"
skills = ["github", "notion", "slack"]

[[view]]
name = "other"
default = false
description = "Additional skills accessible through skill-discover"
skills = ["apple-notes", "blogwatcher"]
```

The default view appears directly in the mounted skill view. Secondary views
are listed by the virtual `skill-discover` skill, whose `SKILL.md` includes the
skill names and source paths.

Skills not assigned to any view are added to the default view on the next
mount.

## Read and Write Semantics

| Operation | Behavior |
| --- | --- |
| `readdir` | Controlled by views and runtime activation state |
| Read `SKILL.md` | Compiled content by default; the selected target's raw content when the directive stage is disabled with no other transform |
| Read ordinary files | Passes through to the physical source tree |
| Write `SKILL.md` | Writes through and reparses the store |
| Write ordinary files | Writes through without changing skill metadata |
| Rename skill directory | Uses the directory name as the authoritative key |
| Symlink or hardlink | Restricted to safe same-skill relative targets |
| `user.*` xattr | Conservative passthrough on ordinary paths |

In-place authoring supports newly created skill directories. A fresh directory
does not expose a phantom `SKILL.md` before the manifest exists; once
`SKILL.md` is written, SkillFS reparses it and exposes the compiled view.
Pending or direct-final installs can preserve ordinary top-level skill
directory metadata such as mode, timestamps, and ownership. `.skill-meta/**`
remains restricted to trusted metadata paths.

Without security integration, skills read from the live source tree. When
security activation is enabled, visibility is constrained by the active
mapping:

- current: read from the live source tree, for example through the legacy
  decision-command resolve path;
- fallback: read from a trusted snapshot under `.skill-meta`;
- hidden: hide the skill from ordinary callers.

In activation file mode, activation JSON expresses fallback and hidden states.
It does not write current/live state. If a skill has no activation JSON or
activation xattr in this mode, SkillFS treats it as hidden by fail-safe default.

## Read-Time Transforms

After the activation target is resolved, `SKILL.md` bytes pass through an
ordered transform pipeline before an agent sees them:

1. The **directive** stage runs the conditional compiler (`@if` / `@else` /
   `@endif` plus heuristic command normalization). It is enabled by default;
   when present it always runs first, so output is unchanged from earlier
   releases. Disable it with `[transforms.directive] enabled = false`.
2. The optional **OS adapter** stage runs second and only on `SKILL.md`. It
   rewrites distribution-specific literals between Ubuntu/Debian and
   Alinux/Anolis conventions.

Both stages are optional: you can run both, directive-only (the default),
adapter-only (directive disabled), or neither — an empty pipeline serves the
selected raw bytes unchanged. Initialization diagnostics report the actual
enabled stage list.

| Directive | OS adapter | Agent-visible `SKILL.md` |
| --- | --- | --- |
| enabled (default) | disabled (default) | Legacy compiler output |
| enabled | enabled | Compiler output, then OS adaptation |
| disabled | enabled | OS adaptation of raw selected bytes |
| disabled | disabled | Raw selected bytes |

The pipeline only affects the bytes an agent reads. Source files, trusted
snapshots, activation metadata, and the rule artifact are never modified.
Hidden skills stay hidden and never enter the pipeline; a fallback read is
transformed from the trusted snapshot and never falls back to the live source.
The same pipeline and activation ordering applies to flat `<skill>/SKILL.md`
and Hermes `<category>/<skill>/SKILL.md` layouts. A snapshot read resolves,
reads, and transforms only the selected snapshot; if snapshot target parsing or
resolution fails, or its `SKILL.md` cannot be read, the operation returns an
error (`ENOENT` at the virtual read boundary) and never retries the live source.
`getattr` size, partial reads, and full reads always agree on the transformed
bytes. Only `SKILL.md` is adapted — other Markdown, shell, Python, and config
files pass through untouched.

### Disabling the Directive Stage

The directive/compiler stage stays enabled unless explicitly turned off:

```toml
[transforms.directive]
enabled = false
```

An absent `[transforms.directive]` section keeps directive compilation enabled,
so existing configurations are unaffected. Disabling it only affects the
compiler stage; the OS adapter remains independently opt-in.

### Enabling the OS Adapter

The OS adapter is disabled by default and configured through the existing
`--config <PATH>` TOML file (no extra CLI flags). When enabled without a
`rules_path`, it uses the built-in catalog:

```toml
# /etc/skillfs/skillfs-security.toml
[transforms.directive]
enabled = true

[transforms.os_adapter]
enabled = true
target_os = "alinux" # auto | ubuntu | alinux
# rules_path = "/etc/skillfs/ubuntu-alinux.custom.yaml"
```

```bash
skillfs mount /path/to/skills /mnt/skillfs \
  --config /etc/skillfs/skillfs-security.toml
```

SkillFS ships a **built-in 312-rule Ubuntu/Alinux catalog** embedded in the
binary from the repository asset, so the adapter works in source builds, RPMs,
and containers without a separate file. It stays opt-in. The catalog contains
257 `auto_apply: always` rules and 55 `auto_apply: never` protection rules,
producing 223 active substitutions toward Alinux and 192 toward Ubuntu. Most
high-confidence rules are applied; medium- and low-confidence rules and unsafe
bare-token matches remain protection-only.

- `target_os = "auto"` reads the exact `/etc/os-release` `ID` once at mount
  startup — `ubuntu`/`debian` map to Ubuntu, `alinux`/`anolis` map to Alinux.
  Detection is fail-closed: `ID_LIKE` is not consulted, so RHEL-family
  derivatives (Rocky, AlmaLinux, CentOS, …) are not silently treated as Alinux,
  and unrecognized hosts reject the mount. Set `ubuntu` or `alinux` explicitly
  on other distributions.
- `rules_path` is an optional external override. Omit it to use the built-in
  catalog; set a non-empty path to load an external read-only artifact instead.
  A present-but-blank path is rejected, not treated as the default. SkillFS
  loads and validates the chosen artifact once at startup; the per-read path
  performs only in-memory substitution and never parses YAML, reads
  `/etc/os-release`, spawns processes, or makes network/LLM calls.
- TOML controls which stages run, the target OS, and the rule artifact. The YAML
  artifact controls individual mappings and eligibility. There is no per-rule
  TOML switch.

### Enabling Protected Rules and Adding Custom Rules

The rule artifact — built-in or external — is a top-level YAML sequence. Each
rule declares the literal for each OS side, a `direction`, and a required
`auto_apply` flag:

```yaml
- ubuntu: "apt-get install -y "
  alinux: "dnf install -y "
  direction: bidirectional          # bidirectional | ubuntu_to_alinux_only | alinux_to_ubuntu_only
  auto_apply: always                # always | never — REQUIRED
```

`rules_path` is a **complete replacement**, not an overlay. To retain all
built-in mappings and customize only selected entries, copy the repository asset
from a source checkout:

```bash
cp src/skillfs/crates/skillfs-core/assets/ubuntu-alinux.yaml \
  /etc/skillfs/ubuntu-alinux.custom.yaml
```

Then set `rules_path = "/etc/skillfs/ubuntu-alinux.custom.yaml"` in the TOML
configuration. An absolute path avoids dependence on the mount process working
directory.

To opt a protected medium- or low-confidence rule into local policy, change its
`auto_apply` value in the copied artifact. For example:

```yaml
- ubuntu: "ufw"
  alinux: "firewalld"
  direction: ubuntu_to_alinux_only
  auto_apply: always
  confidence: low
  notes: "enabled by local policy"
```

Append complete entries to define local mappings:

```yaml
- ubuntu: "acme-agent-dev"
  alinux: "acme-agent-devel"
  direction: bidirectional
  auto_apply: always
  confidence: high
  notes: "local package mapping"
```

`ubuntu`, `alinux`, `direction`, and `auto_apply` are required.
`confidence` and `notes` are optional inert annotations. The external file
must also retain any built-in rules you still want: SkillFS does not merge it
with the embedded catalog. Rules are loaded once when the mount starts; remount
after editing the file. There is currently no catalog overlay, hot reload,
per-rule identifier, or export command.

- `auto_apply` is required on every rule, including external override artifacts;
  only `auto_apply: always` rules are applied, and only in a direction the
  resolved target allows. An artifact that omits `auto_apply` is rejected with an
  error naming the rule index.
- `confidence` and `notes` are accepted as annotations with no behavior —
  eligibility is governed solely by `auto_apply`.
- Substitution is a single non-cascading pass; at each position the longest
  matching pattern wins, so overlapping patterns never chain and file order does
  not affect the result.
- Ineligible patterns (`auto_apply: never`, identity, or direction-disallowed)
  still match and are emitted unchanged, protecting their whole span so a shorter
  eligible rule cannot rewrite inside them. An eligible substitution wins over
  protection for the same source.
- A many-to-one forward mapping must resolve reverse ambiguity explicitly: mark
  one pair `bidirectional` (canonical reverse) and the alternates
  `ubuntu_to_alinux_only`. Colliding `bidirectional` reverses are rejected.

When enabled, a missing/unreadable external `rules_path`, a blank `rules_path`,
malformed YAML, a missing or invalid `direction`/`auto_apply` value, duplicate
or ambiguous patterns, or an unrecognized `target_os = "auto"` host reject the
mount before it starts with an actionable error.

## Security Integration

### Activation File Mode

Use activation file mode when an external daemon receives SkillFS mutation
events, scans the source tree, and writes activation metadata:

```bash
skillfs mount /path/to/skills /mnt/skillfs \
  --foreground \
  --security \
  --activation-mode file \
  --notify-socket /run/skill-ledger.sock \
  --activation-events-log /var/log/skillfs/activation-events.jsonl \
  --activation-reload-mode poll
```

Flow:

```text
Agent or installer writes through SkillFS
  -> SkillFS sends a notify event
  -> Skill Ledger scans and writes activation state
  -> SkillFS reloads activation state
  -> the skill becomes live, fallback, or hidden
```

`--activation-reload-mode poll` requires `--notify-socket` or
`--activation-events-log`, because SkillFS needs a trigger source for polling.

For in-place activation and notify mounts, set `--ledger-backing-root` to a
daemon-visible backing source path:

```bash
skillfs mount /path/to/skills /path/to/skills \
  --security-mode \
  --security \
  --activation-mode file \
  --notify-socket /run/skill-ledger.sock \
  --ledger-backing-root /run/user/$UID/skillfs-ledger/source
```

Avoid `/tmp` and `/var/tmp` for daemon integration paths when the daemon runs
with `PrivateTmp=true`; those paths are invisible to the daemon and rejected by
startup validation.

### Control Socket

The trusted control socket is the preferred production path for activation
writes:

```bash
skillfs mount /path/to/skills /mnt/skillfs \
  --security \
  --activation-mode file \
  --control-socket /run/skillfs/control.sock \
  --trusted-peer-exe /usr/bin/skill-ledger
```

The socket requires `--security --activation-mode file`, is mutually exclusive
with `--decision-command`, and requires a pinned trusted peer executable. Peer
validation uses Linux peer credentials and executable identity checks.

Supported JSONL request examples:

```json
{"schemaVersion":"1","method":"ping"}
{"schemaVersion":"1","method":"status"}
{"schemaVersion":"1","method":"meta.writeActivation","skillName":"demo-weather","activation":{"schemaVersion":1,"target":null}}
{"schemaVersion":"1","method":"meta.setActivationXattr","skillName":"demo-weather","activation":{"schemaVersion":1,"target":null}}
```

### Trusted Mount-path Writer

`--trusted-writer-exe <PATH>` is a compatibility gate for trusted writers that
write through the mount path. Prefer the control socket for new production
integrations.

`--trusted-writer <NAME>` is deprecated and only matches the Linux process
`comm` name. Use executable identity when compatibility allows it.

### Decision-command Mode

`--security --decision-command <COMMAND>` is the legacy compatibility path.
SkillFS invokes the external command for scan and resolve decisions.

Decision-command mode is mutually exclusive with activation file mode,
`--notify-socket`, `--activation-events-log`, `--ledger-backing-root`, and
`--control-socket`.

## Install Protocols

SkillFS supports installer-friendly lifecycle paths:

- staging roots can be hidden from ordinary listing while exact staging paths
  remain writable;
- direct-to-final installs can remain hidden until activation appears;
- `/.skillfs-inbox/<skill>/...` is an install or repair entry point for hidden
  or new skills; writes land in the source tree and can trigger the external
  security flow;
- quiet-timeout notification can aggregate install mutations after a configured
  quiet window;
- post-publish grace can allow bounded installer metadata writes after publish;
- post-publish grace paths for fallback skills are routed to the live source so
  installers can finish metadata updates after publish.

These behaviors are configured through the SkillFS TOML config and require a
notify source such as `--notify-socket` or `--activation-events-log`.

## Observability

### Audit and Activation Logs

`--audit-log <PATH>` writes filesystem audit events as JSONL.
`--activation-events-log <PATH>` writes activation protocol events as JSONL for
daemon-driven activation flows.

### SLS Ops and Runtime Metrics

SkillFS writes best-effort SLS records to:

```text
/var/log/anolisa/sls/ops/skillfs.jsonl
```

The file is owned and pre-created by the deployment/SLS component. SkillFS only
appends when the file exists; it never creates the file or parent directory, and
write failures do not change CLI or FUSE behavior.

The following CLI commands append ops records: `mount`, `list`, `validate`, and
`classify`. While a mount is alive, runtime metric records use
`record_type = "runtime_metric"` and include mount lifecycle, view pruning,
skill hits, and security policy outcomes. The legacy mount-session summary
shares the same file for compatibility.

## Common Options

| Option | Purpose |
| --- | --- |
| `--foreground` | Run in the foreground |
| `--managed` | Start a detached supervised mount |
| `--security-mode` | Require source and mountpoint to be the same path |
| `--security` | Enable security integration |
| `--activation-mode file` | Consume activation JSON/xattr state |
| `--activation-reload-mode poll` | Poll activation after notify triggers |
| `--notify-socket <PATH>` | Send mutation events to an external daemon |
| `--activation-events-log <PATH>` | Write activation protocol events as JSONL |
| `--audit-log <PATH>` | Write filesystem audit events as JSONL |
| `--control-socket <PATH>` | Accept trusted activation write requests |
| `--trusted-peer-exe <PATH>` | Pin the trusted control socket peer |
| `--trusted-writer-exe <PATH>` | Pin a trusted mount-path writer |
| `--ledger-backing-root <PATH>` | Provide a daemon-visible source view |
| `--decision-command <CMD>` | Use legacy external decision mode |
| `--pid-file <PATH>` | Write a process pid file |
| `--allow-other` | Allow other users to access the FUSE mount |
| `--config <PATH>` | Load SkillFS TOML configuration |
| `-v`, `--verbose` | Enable debug logging |
| `--log-file <PATH>` | Write logs to a file |

## Troubleshooting

**A newly installed skill is not visible.**
With security activation enabled, new skills can remain hidden until the ledger
writes activation state. Check notify delivery and activation reload events.

**Fallback reads an older version.**
Fallback intentionally reads a trusted snapshot under `.skill-meta`, not the
live source tree.

**`.skill-meta` is not listed.**
This is expected for ordinary callers. Trusted peers can access metadata through
the configured trusted path.

**Notify socket failures appear in logs.**
Notify failures are warnings and do not stop FUSE service, but the external
daemon may miss mutation events until the socket is fixed.

**In-place activation fails at startup.**
Check that `--ledger-backing-root` is set and visible to the daemon. Avoid
`/tmp` and `/var/tmp` with services that use `PrivateTmp=true`.

**A managed mount survived the launcher restart.**
That is expected. Stop it with `skillfs stop <MOUNTPOINT>`.

## More References

- [SkillFS README](../../../../src/skillfs/README.md)
- [External decision protocol](../../../../src/skillfs/docs/security/external-decision-protocol.md)
- [Runtime activation plan](../../../../src/skillfs/docs/security/runtime-activation-implementation-plan.md)
- [FUSE crate layout](../../../../src/skillfs/docs/architecture/fuse-crate-layout.md)
