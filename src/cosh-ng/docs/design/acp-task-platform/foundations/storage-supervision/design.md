# Phase 0 Storage and Supervision Design

[中文版](design_zh.md) | [Acceptance baseline](acceptance.md) |
[Planning set](../../README.md)

## Status and decisions

- Baseline: `up/main` at `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`
- Status: ADR-S1 is accepted for the local SQLite scope and implemented through storage schema v9;
  remaining exit evidence and ADR-S2 are tracked separately

This module makes two architecture decisions:

1. **ADR-S1:** use an embedded SQLite database in WAL mode, with one local
   application writer, for Task events, projections, idempotency, approvals,
   permits, executions, Runtime bindings, and Outbox delivery.
2. **ADR-S2:** make one Gateway `RuntimeSupervisor` the sole owner of every
   Agent Runtime child process, including cosh-core and ACP Agents. Shell keeps
   ownership of its interactive PTY process only.

These decisions do not merge provider conversation persistence, audit
segments, or terminal evidence into the Task database.

### ADR-S1 implementation note

The current candidate adds `cosh-gateway::storage` with SQLite WAL,
`synchronous=FULL`, foreign keys, `trusted_schema=OFF`, a five-second busy timeout, strict tables,
and one private connection exposed mutably only through `&mut SqliteTaskStore`. It requires an
absolute database path, creates missing dedicated directories as `0700` and the database as
`0600`, rejects relative paths, insecure existing parents, non-regular files, and symlinks in any
existing path component, and validates existing WAL/SHM companions before and after open.

Schema v9 atomically owns Tasks, Task events, actor-scoped command receipts, Outbox intents, and
typed Runtime input request/dispatch rows. The raw Task writer is crate-private in release builds.
Every Task or Outbox payload is limited to 256 KiB and a complete commit to 1 MiB before a
transaction begins. Checksummed migrations, newer-schema refusal, `quick_check`, deterministic event recovery,
full transaction rollback, `SQLITE_FULL` injection, redacted read-only inspection, and no-clobber
online backup/restore have automated evidence. Checkpoint/disk health, the complete kill/power-loss
matrix, corruption quarantine, race-free descriptor-relative open, and universal execution
reconciliation remain required before storage exit. A local SQLite kill-point fixture uses real
`SIGKILL` and proves reopen/replay without partial rows; it is not host power-loss evidence.

## Goals

- Atomically commit command deduplication, Task events, projections, and
  Outbox work.
- Recover Task state after daemon or host restart without treating an Agent
  process as the source of truth.
- Fence output from crashed or replaced Runtime generations.
- Give every child process exactly one owner responsible for spawn, pipes,
  cancellation, escalation, reap, resource bounds, and diagnostics.
- Preserve existing provider-session files and audit storage during migration.
- Support a local-first installation without requiring an external database.

## Non-goals

- Distributed consensus, active-active Gateway replicas, or a network-shared
  SQLite file.
- Storing model transcripts, raw terminal output, secrets, or provider stderr
  in Task events.
- Guaranteeing replay safety for an arbitrary OS side effect. Unknown
  execution outcomes require reconciliation or user approval.
- Supervising the user's native Shell jobs from the Gateway.
- Replacing existing session persistence, audit JSONL, or ws-ckpt protocols.
- Adopting the draft ACP Streamable HTTP transport in Phases 0-2.

## Current-source evidence

| Evidence | Verified baseline behavior | Gap for the target architecture |
| --- | --- | --- |
| [`SessionStore`](../../../../../crates/cosh-core/src/session/store.rs#L40) | Workspace-scoped provider sessions use versioned envelopes, generation checks, locks, and atomic commits | It is not a multi-aggregate Task/Event/Outbox transaction store |
| [`ScopedStorage`](../../../../../crates/cosh-core/src/session/scoped.rs#L27) | Descriptor-relative access, `0700` directories, `0600` files, no-follow opens, and atomic rename harden session files | Equivalent hardening must wrap database creation and backup paths |
| [`PersistedSession`](../../../../../crates/cosh-core/src/session.rs#L83) | Provider transcript and compaction projection are durable | Provider history remains a separate data class from Task state |
| [`CoshCoreService`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service.rs#L47) | Shell owns one persistent core process, its worker thread, cancellation, and restart/reset decisions | Ownership is Shell-local and cannot serve detached Web/channel Tasks |
| [`spawn_provider_child`](../../../../../crates/cosh-shell/src/adapter/process.rs#L66) | Provider children get a separate session/process group, bounded stderr, watchdogs, TERM/KILL, and reap | Logic is split across Shell adapters and is not a durable Runtime supervisor |
| [`output_with_timeout`](../../../../../crates/cosh-core/src/process.rs#L72) | Core also has process-group cleanup for bounded helper subprocesses | There is no single owner for Agent Runtime lifecycle |
| [`Cargo.toml`](../../../../../Cargo.toml) | Candidate declares the workspace SQLite client used by `cosh-gateway` | The focused store slice exists; full ADR-S1 exit evidence remains incomplete |
| [Unified audit design](../../../../../docs/design/audit-log.md) | Audit uses per-process JSONL segments with distinct durability and retention semantics | Audit must not be silently redirected to Task SQLite |

## Data-class boundaries

| Data class | Owner and store | Why separate |
| --- | --- | --- |
| Task lifecycle | Gateway SQLite | Transactional command/event/projection/Outbox invariants |
| Provider conversation | Existing cosh-core `SessionStore` initially | Model transcript, workspace resume, compaction, and provider compatibility |
| Audit | Existing versioned per-process segments | Append-only operational record, independent failure policy and retention |
| Terminal evidence | Shell/evidence owners | Potentially large, short-lived, and referenced by opaque IDs |
| Runtime diagnostics | Supervisor bounded memory plus redacted audit references | Stderr and protocol failures must not enter domain events raw |

Later consolidation requires a separate migration ADR. Phase 1 may reference a
provider session from a Runtime binding but does not copy its messages.

## ADR-S1: SQLite WAL Task store

### Decision

The first Gateway uses a private local database, resolved in this order:

```text
$COSH_GATEWAY_STATE_DIR/state.db
$XDG_STATE_HOME/cosh/gateway/state.db
$HOME/.local/state/cosh/gateway/state.db
```

Parent directories are `0700`; database, WAL, shared-memory, backup, and
migration files are private to the effective user. Opens reject symlinks and
non-regular files. Phase 1 supports local filesystems only. A network or shared
filesystem is unsupported and must fail startup validation rather than degrade
silently.

The database is configured on every connection with equivalent policy:

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA trusted_schema = OFF;
```

`synchronous = FULL` is chosen for Task admission, approval, permit, execution,
and Outbox durability. A measured relaxation to `NORMAL` requires an ADR and a
documented loss window. WAL checkpoint policy is bounded and observable; a
checkpoint failure degrades health and never deletes the WAL.

### One-writer model

- One bounded Gateway writer task owns the write connection.
- All state-changing commands enter this queue after authentication and size
  validation.
- The writer uses `BEGIN IMMEDIATE` and short transactions.
- Read-only projection queries use bounded reader connections and never start
  a write transaction.
- No bridge, presenter, HTTP handler, Shell attachment, or executor holds a
  database connection or writes a table directly.
- Queue saturation returns a stable overload error before admission; callers
  may retry with the same idempotency key.

The single writer is an application ownership rule, not a claim that SQLite
cannot support several writers. It makes ordering, backpressure, migration,
and failure semantics explicit for a local control plane.

### Schema ownership draft

```text
schema_migrations(version, checksum, applied_at_ms)
gateway_meta(key, value)
actors(actor_id, issuer, subject_digest, assurance, status, ...)
commands(command_id, actor_id, idempotency_key, payload_digest,
         accepted_at_ms, result_event_id, ...)
tasks(task_id, owner_actor_id, target_ref, revision, state, ...)
task_events(event_id, task_id, revision, event_type, schema_version,
            payload_json, occurred_at_ms, causation_message_id)
runs(run_id, task_id, attempt, state, terminal_event_id, ...)
external_refs(external_ref_id, kind, authority, scope_digest,
              value_ciphertext_or_value, value_digest)
runtime_instances(runtime_instance_id, generation, launch_digest,
                  state, last_exit_code, ...)
runtime_bindings(binding_id, task_id, run_id, runtime_instance_id,
                 runtime_generation, external_ref_id, state, ...)
approvals(approval_id, task_id, run_id, request_digest, state, ...)
permits(permit_id, approval_id, task_id, run_id, target_digest,
        operation_digest, expires_at_ms, consumed_by_execution_id, ...)
executions(execution_id, permit_id, state, idempotency_scope,
           result_digest, evidence_ref, ...)
outbox(delivery_id, event_id, sink_kind, sink_ref_digest, state,
       attempt, next_attempt_at_ms, lease_owner, lease_expires_at_ms, ...)
```

`task_events` has unique `(task_id, revision)` and immutable rows. Projection
tables are updated in the same transaction and can be rebuilt from events plus
explicitly versioned migration logic. Event payloads use the frozen contract
schema and are bounded before serialization.

### Transaction contracts

#### Command admission

One transaction:

1. insert or verify the scoped idempotency row;
2. read and compare Task revision;
3. append one or more Task events;
4. update Task/Run/Approval/Execution projection rows;
5. enqueue presentation and Runtime dispatch intents in Outbox;
6. commit, then wake workers.

No Runtime call or OS side effect occurs inside the database transaction.

#### Execution boundary

1. Atomically verify and consume a single-use Permit and create
   `Execution(state=starting)`.
2. Execute outside the transaction through `ExecutionTargetPort`.
3. Persist the terminal result and Task event in one transaction.
4. If the process or host crashes after step 1, recovery marks the Execution
   `outcome_unknown`; it does not repeat a non-proven-idempotent mutation.

#### Outbox delivery

Workers lease bounded batches. A delivered sink acknowledgement atomically
marks the row delivered. Lease expiry allows retry with the same `DeliveryId`.
Consumers must deduplicate by Delivery ID or accept at-least-once delivery.

### Migration and recovery

- Migrations are ordered, checksum-pinned, transactional when SQLite permits,
  and applied only by the writer owner before serving traffic.
- A binary refuses a database with a newer schema version.
- Startup runs bounded metadata validation and `quick_check`; full integrity
  checks are an explicit maintenance operation.
- Before destructive migrations, create a verified SQLite online backup in a
  private sibling path and record the restore procedure.
- A failed migration keeps the daemon unavailable; no partial read-only mode
  may approve or execute work.
- On restart, in-flight Runtime bindings are fenced, expired Outbox leases are
  reclaimed, and uncertain executions require reconciliation.

### Alternatives considered

| Alternative | Strength | Why not selected for Phase 1 |
| --- | --- | --- |
| Append-only JSONL plus rebuilt projections | Simple inspection; aligns with audit segments | Atomic event/projection/idempotency/Outbox updates and indexed concurrency require substantial custom recovery |
| One atomic JSON file per Task | Reuses SessionStore patterns | Cross-Task queries, Outbox leasing, uniqueness, and multi-entity transactions become lock choreography |
| Existing `SessionStore` | Mature secure file handling | Its aggregate is provider conversation history, not Task/Approval/Execution state |
| Embedded KV (`redb`, `sled`, RocksDB) | Fast key-value access | Adds custom schema/index/transaction tooling and weaker operational inspectability for this relational workload |
| Rollback-journal SQLite | Simpler file set | Readers block more often during writes; WAL better fits attachment replay and dashboards |
| External PostgreSQL | Strong multi-node operations | Violates zero-dependency local-first installation and is unnecessary before multi-replica control plane requirements |
| In-memory state with audit replay | Small initial implementation | Loses durable idempotency and confuses audit with the authoritative event store |

SQLite WAL does not solve distributed ownership. If a future architecture
requires active-active Gateways or network storage, migrate through a new
storage-port implementation and data migration ADR.

## ADR-S2: Runtime Supervisor ownership

### Decision

`RuntimeSupervisor` is the sole implementation allowed to manipulate handles
for cosh-core and ACP Agent child processes. A bridge owns protocol codecs and
session semantics and may compose exactly one supervisor, as the current
`AcpV1RuntimeBridge` does. Spawn, signal, wait, and reap behavior must still be
delegated to that embedded supervisor; no second lifecycle owner may retain a
child handle.

| Process/resource | Sole owner | Notes |
| --- | --- | --- |
| Native bash/zsh PTY and foreground jobs | `cosh-shell` Shell host | Preserves terminal job control and attach experience |
| cosh-core Agent Runtime | Gateway `RuntimeSupervisor` after Phase 1 migration | Current Shell owner remains only during compatibility fallback |
| ACP Agent stdio process | Gateway `RuntimeSupervisor` | Phase 2 local stdio only |
| Short-lived core helper subprocess | Its existing scoped core owner | Not an Agent Runtime; must retain process-group cleanup |
| OS operation execution | `ExecutionTargetPort` implementation | Governed by Permit; not a Runtime bridge child |

### Typed supervision contract

```rust
struct RuntimeLaunchSpec {
    kind: RuntimeKind,
    executable: TrustedExecutable,
    args: Vec<BoundedArg>,
    cwd: CanonicalPath,
    env: AllowlistedEnvironment,
    protocol: RuntimeProtocol,
    resource_profile: ResourceProfile,
    restart_policy: RestartPolicy,
}

struct SupervisedRuntimeRef {
    instance_id: RuntimeInstanceId,
    generation: u64,
    launch_digest: Digest,
}

enum SupervisorCommand {
    EnsureRunning { spec: RuntimeLaunchSpec },
    OpenChannel { runtime: SupervisedRuntimeRef },
    CancelRun { runtime: SupervisedRuntimeRef, run_id: RunId },
    Stop { runtime: SupervisedRuntimeRef, reason: StopReason },
}

enum SupervisorEvent {
    Started { runtime: SupervisedRuntimeRef, pid_observed: u32 },
    Ready { runtime: SupervisedRuntimeRef },
    ProtocolFailed { runtime: SupervisedRuntimeRef, code: RuntimeErrorCode },
    Exited { runtime: SupervisedRuntimeRef, exit: BoundedExit },
    RestartScheduled { previous: SupervisedRuntimeRef, backoff_ms: u64 },
}
```

PID is diagnostic only. `RuntimeInstanceId + generation` is the fencing
identity. Secrets are referenced from a credential provider and materialized
only into the child launch environment; they are absent from launch digests,
events, and diagnostics.

### State machine

```text
Absent -> Starting -> Initializing -> Ready <-> Busy
             |             |           |        |
             +-------------+-----------+--------+-> Stopping -> Exited
                                      \----------> Failed -> Backoff -> Starting(new generation)
```

- Every spawn increments generation before any event can be admitted.
- `Ready` requires protocol initialization and version/capability validation.
- Bridges may multiplex sessions only when the negotiated protocol and
  scheduler policy permit it. One ACP connection can support several sessions,
  but Phase 2 does not assume every Agent safely handles concurrent prompts.
- cosh-core process reuse continues to respect its current approval mode,
  workspace scope, and provider-session binding constraints.
- Unexpected exit makes every binding for that generation stale. Task state
  remains durable and decides resume, retry, or user intervention.
- Restart budgets use bounded exponential backoff and a circuit-open terminal
  health state; crash loops never spin indefinitely.

### Spawn and I/O safety

- Resolve executables through trusted installation/configuration, not user
  prompt text. Record an executable/argument digest.
- Canonicalize cwd and validate target access before spawn.
- Start the child in its own process group/session; do not use PID as a durable
  identity.
- Use piped stdin/stdout for protocols, continuous bounded stderr draining,
  maximum line/message sizes, bounded queues, and explicit backpressure.
- Protocol stdout must contain protocol frames only. Human logs go to stderr
  and are redacted/bounded before diagnostic retention.
- Close-on-exec all unrelated descriptors. Child environment is allowlisted;
  Gateway/channel credentials are never inherited by default.
- Register the child as owned only after pipes and process-group setup succeed;
  every partial-spawn failure still kills and reaps it.

### Cancellation and shutdown

For an active Run:

1. persist `CancelRequested` in the Task store;
2. bridge sends protocol cancellation when available (`session/cancel` for an
   ACP prompt; current core interrupt for cosh-core);
3. wait a bounded protocol grace while accepting allowed terminal updates;
4. close stdin or send shutdown when the connection is being retired;
5. send `SIGTERM` to the process group;
6. after a bounded grace, send `SIGKILL` to the process group and direct child;
7. reap the child and reader tasks;
8. persist the observed cancellation/exit outcome with the same Runtime
   generation.

Daemon shutdown stops admissions, durably records pending cancellation or
handoff state, drains Outbox within a deadline, terminates Runtime children,
and closes SQLite last. Shell PTY shutdown remains owned by Shell.

### Restart and orphan policy

- Gateway child processes are not intentionally orphaned across daemon exit.
- On daemon restart, durable Runtime instances become `stale`; the supervisor
  does not attach to a PID discovered by number alone.
- A future detach/reattach model for Runtime processes requires a brokered
  socket, authenticated ownership token, and separate ADR.
- Tasks with no side effect may resume/retry according to bridge capability.
  Executions in `starting` or `running` require target-specific reconciliation
  before retry.

### ADR-S3: hard-crash Runtime containment

**Status: selected and fixture-verified for the first backend; overall
production Runtime admission is not accepted.** Linux systemd cgroup ownership
is the selected first deployment backend. Unmanaged
Linux and macOS have no accepted Phase 1 backend and must fail closed for the
durable Runtime scheduler.

`RuntimeSupervisor` owns protocol shutdown, process-group escalation, and reap
only while the Gateway process is alive. `SIGKILL` prevents all of those paths,
including `Drop`, from running. Durable leases and generation fences reject
stale database mutation, but they do not terminate an existing OS process.
Process groups also have no automatic parent-death lifecycle.

Hard-crash containment therefore requires an independent lifecycle owner to
terminate every local Runtime descendant after Gateway death, including a
descendant that ignores `TERM`, double-forks, or creates another session. This
does not roll back an OS or remote effect that already started. Such an effect
remains uncertain, is never replayed automatically, and requires typed
reconciliation or operator intervention.

Runtime admission requires an opaque `VerifiedRuntimeContainment` proof that
only a platform verifier can create. A CLI flag, environment variable,
configuration claim, PID file, process group, or database row cannot create
this proof. The production `serve` path verifies containment before binding its
socket, starting the scheduler, committing a Task, or spawning a Runtime. A
missing or invalid proof returns `runtime_containment_unverified` without a
Runtime-side effect.

The first Linux backend uses a dedicated externally owned systemd service
cgroup. The verifier must confirm that the current process belongs to the
configured live unit and that its effective properties provide control-group
kill semantics with unconditional final `SIGKILL`, `Type=exec` main-process
tracking, and no delegation to Runtime descendants. Main-process death must
therefore enter the unit stop path that kills the remaining cgroup. A replacement
Gateway does not publish readiness until the previous unit cgroup is empty.
Graceful process-group cleanup remains mandatory defense in depth, but is not
the hard-crash proof.

The following alternatives do not independently satisfy the decision:

- `PDEATHSIG` covers only a direct child and has installation/fork races;
- pidfds and subreapers improve observation or reap ownership but do not
  automatically kill all descendants;
- a cgroup managed only by Gateway has no surviving owner to invoke
  `cgroup.kill` after Gateway dies;
- a per-Runtime guardian needs a separate ADR covering its own death,
  authentication, and recovery;
- a PID namespace or container lifecycle owner may become an equivalent future
  backend after the same kill-point evidence passes.

Acceptance requires unverified launches to fail before admission, an opaque
proof with no production test escape, and an isolated systemd fixture that
`SIGKILL`s Gateway while direct children, ignore-`TERM` grandchildren,
double-forked descendants, and `setsid` descendants are active. Restart must
wait for the old cgroup to empty, must not attach by PID, and must converge
prompt, permission, and started-effect crash windows without replay. Default
tests must not install, start, or modify a host service.

The gated destructive fixture reported PASS in a disposable Ubuntu 24.04 arm64
container with systemd 255. It rendered the packaged unit, verified its live
effective containment properties, proved a same-UID user-manager positive
control while rejecting the Gateway descendant escape, killed the Gateway main
PID, and observed direct child, grandchild, double-fork, and `setsid` cleanup
before replacement readiness. This evidence is limited to that environment and
is not tied to an exact candidate commit.

## Error and security boundaries

- Storage unavailable, migration failure, corrupt critical rows, or schema
  mismatch blocks new governed execution.
- Read-only UI may expose an explicit degraded state only when doing so does
  not mutate leases or acknowledge delivery.
- Database errors carry stable safe codes; SQL, paths containing private data,
  payload JSON, and secrets are not returned to channels.
- Supervisor error events contain bounded exit class and redacted stderr
  reference, never raw streams.
- A bridge cannot bypass Broker authorization through terminal or filesystem
  callbacks.
- Runtime generation and launch digest are verified on every event and
  permission response.
- Database backup/export requires explicit authorization and private output
  permissions.

## Compatibility and migration

1. Add SQLite store and Runtime Supervisor behind new ports without changing
   current Shell behavior.
2. Implement CoshCore Bridge under Supervisor control; keep current Shell-local
   service as a feature-gated fallback.
3. Persist Task-to-existing-provider-session bindings; do not import transcript
   messages into SQLite.
4. Switch Shell to Gateway attachment only after Phase 1 storage/restart gates
   pass.
5. Add ACP Runtime under the same Supervisor in Phase 2.
6. Retire duplicate Shell Agent process ownership after the compatibility
   window, while Shell retains native PTY ownership.

Rollback before final cutover disables Gateway admission and returns to the
current Shell-local path. Database files are preserved for forward recovery;
rollback code must not downgrade or rewrite a newer schema.

## Dependencies

- [Protocol contracts](../protocol-contracts/design.md) defines stored event
  and supervisor port payloads.
- [Identity and correlation](../identity-correlation/design.md) defines
  foreign-key identity and generation fencing.
- Phase 1 Task Plane implements the writer and reducer.
- Phase 1 CoshCore Bridge is the first supervised Runtime.
- Phase 2 ACP bridge consumes supervised stdio.

## Implementation tasks

1. Record ADR-S1 and ADR-S2 acceptance, including local-filesystem support.
2. Select a maintained SQLite Rust crate under workspace dependency policy.
3. Implement secure state-path creation, connection policy, migrations, writer
   queue, readers, health, backup, and restore tooling.
4. Implement schema 1, atomic command/event/projection/Outbox transactions,
   and crash recovery.
5. Implement supervisor state machine, process groups, bounded I/O, generation
   fencing, restart budgets, and shutdown ordering.
6. Move CoshCore Bridge process ownership behind the Supervisor.
7. Add fake core and ACP child fixtures for crash, hang, malformed output,
   cancellation, and process-tree leakage.
8. Document operational status, backup, restore, corruption, and disk-full
   procedures before enabling the Gateway by default.

## Test strategy

- SQLite tests cover transaction rollback, unique revisions, foreign keys,
  idempotency conflicts, Outbox leases, migration checksums, disk full,
  checkpoint failure, corruption, backup, and restore.
- Crash fixtures stop the process after each transaction boundary and verify
  replay/reconciliation behavior.
- Concurrency tests saturate writer and reader queues without bypassing the
  sole writer.
- Supervisor tests cover partial spawn, invalid initialization, huge lines,
  closed pipes, stderr floods, timeout, TERM-ignoring children, grandchildren,
  crash loops, shutdown, and stale-generation output.
- No test invokes privileged OS mutation. Process tests use deterministic
  fixture programs and temporary directories.

## Open decisions

| Decision | Owner | Must close by |
| --- | --- | --- |
| SQLite Rust crate and feature set | Storage owner | First Phase 1 storage PR |
| Exact WAL auto-checkpoint and maximum WAL health thresholds | Storage/SRE owners | Before restart acceptance |
| Encryption mechanism for raw external reference values | Security owner | Schema migration 1 freeze |
| Runtime pool concurrency per Agent implementation | Runtime owner | Bridge-specific acceptance |
| Linux pidfd/subreaper use versus process-group baseline | Runtime owner | Supervisor implementation review |
| Database retention/compaction policy for Task events | Product and storage owners | Before public Gateway rollout |
