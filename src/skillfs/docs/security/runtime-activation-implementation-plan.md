# SkillFS Runtime Activation Implementation Plan

Status: A1/A2/A3/A4/A5/A6/B1 implemented

This document records the production-oriented path from the current
`--decision-command scan -> resolve` integration toward a daemon-driven
runtime activation contract. It is a SkillFS implementation plan. The
external security provider owns scanning, policy evaluation, versioning,
and activation decisions; SkillFS owns filesystem observation, safe target
validation, event delivery, and view exposure.

## Current State

SkillFS currently supports two security integration paths.

The compatibility path keeps the external decision command in the SkillFS
process:

```text
FUSE mutation
  -> per-skill debounce
  -> <decision-command> scan <skill_dir> --json
  -> <decision-command> resolve <skill_dir> --json
  -> update in-memory ActiveSkillResolver
  -> expose /skills/<name> as current, fallback, or hidden
```

The production-oriented path delegates security work to an external daemon and
has SkillFS consume only the resulting activation state:

```text
FUSE mutation
  -> per-skill debounce
  -> append protocol event log
  -> notify external daemon over Unix socket

External daemon
  -> debounce/reconcile/check/scan/policy
  -> write .skill-meta/activation.json and optional activation xattr

SkillFS
  -> consume activation state
  -> update ActiveSkillResolver
  -> expose /skills/<name> as snapshot or hidden
```

The decision-command path remains useful for CLI-based integration and demo
validation. The activation path is the preferred shape for daemon integration
because activation state is durable outside the SkillFS process.

## Design Principles

- Directory name remains the canonical SkillFS identity. `SKILL.md name:` is
  metadata only and must not create an alias.
- SkillFS must fail safe. Invalid, missing, inconsistent, or unsafe activation
  state hides the skill instead of exposing live source.
- SkillFS must not parse policy, findings, scan status, or ledger internals.
  It only consumes the runtime activation target.
- Read paths should continue using the in-memory `ActiveSkillResolver`; do not
  read activation files or xattrs on every FUSE read.
- Mutating writes still land in the source/current workspace. Snapshots are
  served read-only through active mapping and fd pinning.
- `.skill-meta/**` changes must not create notification loops.
- The existing `--decision-command` path remains available until daemon mode is
  fully validated and explicitly deprecated.

## Runtime Activation Contract

The primary file contract is:

```text
<skill_dir>/.skill-meta/activation.json
```

The JSON payload is intentionally small:

```json
{
  "schemaVersion": 1,
  "target": ".skill-meta/versions/v000001.snapshot"
}
```

No active runtime version is represented as:

```json
{
  "schemaVersion": 1,
  "target": null
}
```

SkillFS validation rules:

- `schemaVersion` must be exactly `1`.
- `target = null` maps to `ActiveTarget::Hidden`.
- Non-null `target` must be a relative path under
  `.skill-meta/versions/<version>.snapshot`.
- Reject absolute paths, empty strings, `.` / `..` traversal, non-snapshot
  targets, foreign roots, malformed JSON, unsupported schema versions, and
  unknown unsafe shapes.
- The resolved snapshot directory must exist and must stay within the owning
  `skill_dir`.
- Invalid activation maps to hidden and produces a diagnostic error; it must
  not panic or expose live source.

A2 implements the xattr activation contract. The xattr name is:

```text
user.agent_sec.skill_ledger.activation
```

The xattr is preferred when present; `activation.json` is the fallback when
the xattr is absent or the filesystem does not support user xattrs. If both
exist and disagree, SkillFS fails safe and hides the skill. If `lgetxattr`
returns an unexpected error (e.g. `EACCES`, `EIO`), SkillFS fails safe and
does not fall back to `activation.json`.

## Implementation Packages

### A1: Activation File Consumer

Goal: consume `.skill-meta/activation.json` and initialize or refresh
`ActiveSkillResolver` without invoking the external decision command.

Scope:

- Add `security::activation` with strict `ActivationRecord` parsing.
- Add helpers to validate target paths and convert activation into
  `ActiveTarget::Snapshot` or `ActiveTarget::Hidden`.
- Add startup loading behind an explicit opt-in CLI/config setting.
- Preserve existing `--decision-command scan -> resolve` behavior when the new
  setting is absent.
- Add unit and integration tests for valid, hidden, invalid, missing, and
  unsafe activation states.

Out of scope for A1:

- xattr activation consumption.
- daemon notify.
- event log schema changes.
- reconcile loop.
- policy computation or parsing ledger internals.

### A2: Activation Xattr Fallback

Status: **implemented**

Priority: immediate follow-on to A1. Required before N2/E1/R1.

Goal: consume `user.agent_sec.skill_ledger.activation` with
`activation.json` fallback.

Scope:

- Read the user xattr without following symlinks via direct `lgetxattr`
  libc call against the physical source directory, rather than through
  the FUSE xattr callback path. This avoids notification loops and keeps
  activation consumption separate from ordinary user xattr passthrough.
- Parse the same `ActivationRecord` payload.
- Prefer xattr when present and valid.
- Fall back to `activation.json` only when xattr is absent (`ENODATA`)
  or the filesystem does not support user xattrs (`ENOTSUP`/`EOPNOTSUPP`).
- If xattr exists but is invalid (bad JSON, unsupported schema, bad
  target), fail-safe hidden with **no fallback** to `activation.json`.
- If both xattr and `activation.json` exist and are valid but their
  `target` fields disagree, fail-safe hidden.
- `bootstrap_activation` uses the prefer-xattr path by default.

Out of scope for A2:

- Daemon notify (`skill_ledger.skillfs_notify_change`).
- Protocol event log.
- Reconcile loop.
- CLI/config changes (no new flags or modes).
- FUSE read-path changes (still uses in-memory `ActiveSkillResolver`
  populated at startup; no per-read xattr calls).

### A3: Notify Change, Protocol Event Log, And Runtime Reload

Status: **implemented**

Goal: notify the external daemon that a skill source workspace may have
changed, record protocol-visible events, and refresh the active resolver after
the daemon writes activation state.

Scope:

- Add a Unix socket client for `skill_ledger.skillfs_notify_change`.
- Send one NDJSON request frame per debounced skill change.
- Include `schemaVersion`, `skillDir`, `skillName`, `eventKind`, and relative
  `paths`.
- Treat successful send as event acceptance, not as security approval.
- On failure, write diagnostics and keep serving the existing trusted mapping.
- Add a separate JSONL writer for the protocol event schema.
- Fields: `schemaVersion`, `time`, `skillDir`, `skillName`, `eventKind`,
  `paths`.
- Keep it separate from the existing audit stream and security event stream.
- Do not rely on this log as the only source of truth; daemon reconcile must
  re-read current disk state.
- After successful notification, reload activation on an explicit trigger or
  bounded delay.
- On SkillFS startup, load activation for every managed skill.
- Provide an explicit refresh API for future daemon ack integration.
- Preserve fd-pinned read consistency for already opened handles.

### A4: Startup Reconcile And Reload Observability

Status: **implemented**

Goal: close the daemon-restart and missed-change gap with startup reconcile
notifications, and make activation reload outcomes visible in the protocol
event log.

Scope:

- Emit best-effort startup reconcile notifications for known skills after the
  mount is ready.
- Run reconcile on a background thread so mount startup is not blocked by a
  slow or unavailable daemon.
- Add `reloadOutcome` to protocol events for activation reload results:
  `activation_updated`, `activation_unchanged`, `activation_timeout`, and
  `activation_invalid_hidden`.
- Provide explicit runtime reload helpers for one skill or known skills.
- Preserve fd-pinned read consistency: old file handles keep their open-time
  target; new opens observe the updated active mapping.

### A5: Activation State Watcher And Continuous Convergence

Status: **implemented**

Goal: make SkillFS continuously converge its in-memory `ActiveSkillResolver`
to the daemon-owned activation state, even when the activation update was not
produced by the current mount's notify/poll cycle.

Problem statement:

The activation path currently uses startup bootstrap plus notify-triggered,
bounded reload polling. That is sufficient for the happy path, but it is not a
continuous subscription to the activation authority. SkillFS can keep serving a
stale hidden/current/fallback view when:

- SkillFS mounts before the daemon writes activation; daemon reconcile writes
  activation later, but the current mount never reloads it.
- A normal notify is delivered, but scan/resolve takes longer than
  `poll_reload_skill()` timeout; the later activation write is missed.
- Daemon startup reconcile, config change, manual operator action, or another
  control-plane flow updates activation without being triggered by this mount.
- Notify socket delivery fails, then the daemon later repairs state through
  its own reconcile path.
- Startup reconcile only sends notify; if no reload is attached to that
  reconcile, the activation written by the daemon is not reflected in memory.
- A source mutation notification is missed or filtered, but the daemon still
  writes a new activation through another path.

Scope:

- Add an activation-state observer for every managed skill. It watches the
  activation authority, not arbitrary source content.
- Observe `<skill>/.skill-meta/activation.json` mtime and the owning skill
  directory ctime, reusing the composite freshness model already used for
  xattr-aware reload.
- On freshness advance, call `load_activation_prefer_xattr()` and update the
  `ActiveSkillResolver` with the same fail-safe hidden semantics as A1/A2/A3.
- Treat notify-triggered `poll_reload_skill()` as the fast path, not the only
  path. A5 should eventually catch activation changes after poll timeout.
- Attach reload to startup reconcile: after emitting reconcile notifications,
  schedule reload/poll for the reconciled skills so the current mount can pick
  up daemon-written activation.
- Provide a fallback periodic activation reload when filesystem events are
  unavailable or unreliable. The interval should be configurable and low-frequency; this is
  an eventual-consistency repair loop, not a per-read check.
- Register newly discovered or inbox-installed skills into the activation
  observer set so new activations can be consumed without remounting.
- Emit protocol diagnostics for watcher reload outcomes where useful, without
  changing FUSE errno or mutating source data.

Out of scope for A5:

- Parsing `latest.json`, findings, policy, or scan status.
- Running scan/check inside SkillFS.
- Reading activation state on every FUSE read.
- Changing fd pin semantics. Already-opened handles keep their pinned target;
  new `lookup`/`open`/`readdir` operations use the refreshed resolver.
- Expanding source-tree watcher coverage beyond activation state convergence.

Acceptance criteria:

- If SkillFS starts hidden because activation is missing, and activation is
  written later, the mounted view converges without remount.
- If notify-triggered poll times out and activation is written after the
  timeout, the watcher or periodic repair loop still refreshes the resolver.
- If daemon reconcile or operator action updates activation without a FUSE
  mutation, SkillFS eventually observes the update.
- If the notify socket is unavailable and daemon repair happens later, SkillFS
  eventually observes the repaired activation.
- Startup reconcile can lead to a current-mount resolver refresh once daemon
  activation is written.
- Invalid or inconsistent xattr/json activation still hides the skill.
- Existing decision-command mode and activation reload fast path remain
  backwards compatible.

### A6/B1: Ledger Backing Root For Source-Side Security Work

Status: **implemented**

Goal: decouple the agent-visible FUSE view from the external ledger's
source-side working path, especially for in-place security mounts.

Problem statement:

In an in-place mount, the original skill source path is over-mounted by
SkillFS. That is the desired agent-facing boundary: reads can be hidden,
served from a fallback snapshot, or served from the current source depending
on activation state. The external security daemon, however, must scan and
version the live source tree. If it scans the same over-mounted path, a hidden
skill may be invisible and a fallback skill may appear as an older trusted
snapshot. The trusted-writer gate only controls selected `.skill-meta/**`
mutations through the FUSE path; it does not give another process access to
the original live source behind the in-place mount.

The implemented solution is a ledger backing root: a private source alias
prepared before the in-place mount becomes active. SkillFS continues to expose
the normal mount path to agents, while the external security daemon scans and
writes activation state through the backing root.

Conceptual layout:

```text
agent-visible path
  -> SkillFS FUSE view

ledger backing root
  -> private alias of the live source tree

external daemon
  -> scans <ledger-backing-root>/<skill>
  -> writes .skill-meta/activation.json and activation xattr

SkillFS
  -> sends notify skillDir under the ledger backing root
  -> bootstraps/reloads activation from the same backing root
  -> exposes hidden/current/fallback through the FUSE view
```

Scope:

- Add a first-class `ledger_backing_root` / `ledger_work_root` concept to the
  security mount configuration.
- For in-place mounts, require or automatically create a backing root before
  the FUSE over-mount, using a private bind mount or equivalent source alias.
- For non-in-place mounts, allow the same concept to be used as a normalized
  daemon working path. It may point at the source directly or at a private
  alias, but SkillFS should present one consistent path shape to the daemon.
- Use the backing root for notify `skillDir`, activation bootstrap,
  activation reload, startup reconcile, activation watching, and any future
  source-side daemon-facing event payloads.
- Keep the agent-visible FUSE path unchanged. Agents should not need to know
  whether a backing root exists.
- Validate that the backing root is outside the agent-visible mount path and
  does not resolve through the FUSE view.
- Create or validate the backing root under a private parent directory with
  owner-only access. Treat it as a privileged management entry point, not as
  a user-visible path.
- On shutdown, clean up any bind mount and temporary directory that SkillFS
  created.
- Keep `.skill-meta/**` trusted-writer semantics separate. Trusted-writer
  controls the FUSE entry point; backing-root access is controlled by OS
  permissions and mount setup.

Out of scope for A6/B1:

- Changing activation JSON or xattr schema.
- Changing active mapping or fd pin behavior.
- Letting ordinary agents access the backing root.
- Replacing the trusted-writer gate.
- Passing pre-opened source fds to the daemon. That is a possible future
  hardening step, but it is more complex than the backing-root rollout.

Acceptance criteria:

- In-place security mount can expose an activated FUSE view while the daemon
  scans the live source through the backing root.
- A hidden skill remains hidden through the FUSE path but is still visible to
  the daemon through the backing root.
- A fallback skill serves the trusted snapshot through the FUSE path while
  the daemon still scans the live current source through the backing root.
- Notify payloads use the backing-root `skillDir`; the daemon does not need
  to infer in-place vs non-in-place mount mode.
- Activation bootstrap, reload, reconcile, and activation watching all use
  the same backing root.
- Backing-root setup fails closed when ownership, parent permissions, path
  shape, identity, or bind-mount setup is unsafe.
- Non-in-place security mounts can opt into the same backing-root path shape
  for daemon consistency without changing ordinary passthrough semantics.

## CLI And Config Direction

The current CLI surface is:

```text
--security
--decision-command <COMMAND>
--activation-mode off|file
--notify-socket <PATH>
--activation-events-log <PATH>
--activation-reload-mode off|poll
--events-log <PATH>
--trusted-writer-exe <PATH> # production: exe identity (dev,ino) pinning
--trusted-writer <NAME>     # deprecated / compatibility; process comm is spoofable
--ledger-backing-root <PATH>
--config <PATH>
```

The production activation path should be explicit and opt-in while it is being
introduced. Suggested configuration shape:

```toml
[activation]
mode = "file"        # off | file
reload = "poll"      # off | poll
reload_interval_ms = 250
reload_timeout_ms = 5000

[notify]
mode = "off"         # off | unix-socket
socket_path = "/run/user/1000/agent-sec-core/daemon.sock"

[activation_events]
log_path = "/var/log/skillfs-activation-events.jsonl"

[ledger]
backing_root = "/run/skillfs-ledger/<mount-id>/source"
```

Do not silently switch existing `--security --decision-command` users to the
activation path. Compatibility is important during security-side rollout.

## Acceptance Criteria

A2 is complete when:

- All A1 acceptance criteria still pass (json-only activation unaffected).
- Xattr-only activation (no `activation.json`) resolves the snapshot.
- Missing xattr falls back to `activation.json` transparently.
- Invalid xattr hides the skill even when `activation.json` is valid.
- Xattr/json target mismatch hides the skill.
- Unsupported-xattr environments fall back to `activation.json` (tests
  deterministically skip when the substrate lacks `user.*` support).
- FUSE read path still uses the startup-loaded `ActiveSkillResolver`;
  no per-read xattr calls.

A1 is complete when:

- Valid activation snapshot maps `/skills/<name>` to the snapshot tree.
- `target = null` hides the skill from `readdir` and `lookup`.
- Invalid target shapes hide the skill and produce a diagnostic error.
- Missing activation hides the skill only when activation mode is enabled.
- Existing `--decision-command` behavior is unchanged when activation mode is
  disabled.
- Fallback snapshot reads continue to respect fd pinning.

Minimum validation:

```text
cargo check -p skillfs-fuse -p skillfs
cargo test -p skillfs-fuse --lib security::activation
cargo test -p skillfs-fuse --lib activation_reload
cargo test -p skillfs-fuse --lib security::notify
cargo test -p skillfs-fuse --lib security::protocol_events
cargo test -p skillfs-fuse --test ledger_active_mapping_tests
cargo test -p skillfs-fuse --test ledger_demo_refresh_tests
cargo test -p skillfs-fuse --test notify_client_tests --test notify_fuse_tests
cargo test -p skillfs-fuse --tests
cargo test -p skillfs-core
```

## Known Risks

- Reading activation on every FUSE read would be simple but too expensive and
  can break fd consistency. Prefer explicit reload into `ActiveSkillResolver`.
- Xattr and JSON disagreement must not pick one arbitrarily. Hide and record a
  diagnostic event.
- Notify success does not imply scan completion. Do not expose a new version
  until activation state changes safely.
- The daemon can miss events while down. Reconcile must be based on disk state,
  not solely on event log completeness.
- Without A5, activation mode is a startup-plus-triggered-reload cache. It can
  temporarily diverge from daemon-written activation when updates happen after
  a poll timeout or outside the current mount's notify path. A5 is the planned
  convergence mechanism.
- Without A6/B1, in-place activation mode still needs careful deployment:
  external daemons must not scan the over-mounted FUSE path when they need the
  live source. A private backing root is the mechanism for making that
  path split explicit and testable. A6/B1 is now implemented: the
  `--ledger-backing-root` flag and `[ledger].backing_root` config enable
  a private source alias for daemon-facing operations.
