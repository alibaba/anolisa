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
| Read `SKILL.md` | Returns compiled content, not raw source text |
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
