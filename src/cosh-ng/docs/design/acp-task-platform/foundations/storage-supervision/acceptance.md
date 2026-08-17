# Phase 0 Storage and Supervision Acceptance Baseline

[中文版](acceptance_zh.md) | [Design](design.md) |
[Planning set](../../README.md)

## Baseline result

**ADR direction accepted for planning; implementation readiness not accepted.**
The inspected source is
`6c115aefe04ace0d169a24fa7cd55ad7c1befa52`.

The baseline has secure provider-session file persistence and several mature
process-tree cleanup paths. It has no SQLite dependency, Gateway Task store,
Outbox, Runtime Supervisor, daemon recovery, or generation fencing.

## First ADR-S1 implementation result

**Storage result: VERIFIED FIRST SLICE; STORAGE EXIT NOT ACCEPTED.** The current working-tree
candidate implements the Task transaction and local SQLite connection policy. Runtime supervision
is evaluated separately and the root integration report owns its final status.

Reproducible scoped commands are:

- `cargo test --locked --package cosh-gateway storage --no-fail-fast`.
- `cargo test --locked --package cosh-gateway task::aggregate --no-fail-fast`.
- `cargo clippy --locked --package cosh-gateway --lib -- -D warnings`: passed.
- Automated evidence covers WAL/FULL/foreign-key policy, actor and revision substitution, atomic
  Task/Event/receipt/Outbox rollback, checksummed and newer-schema failure, deterministic reopen
  recovery, causation rows, relative paths, insecure parents without chmod, and intermediate or
  final symlinks. Later focused evidence adds `SQLITE_FULL` rollback, redacted read-only
  inspection, source-bound no-clobber online backup/restore, schema v9 typed Runtime input rows,
  and transaction-preflight writer bounds of 256 KiB per payload and 1 MiB per commit.

Result vocabulary: `PASS` is complete reproducible evidence; `PARTIAL` is a verified production
slice with named gaps; `NOT IMPLEMENTED` or `Missing` means no production path; `BLOCKED` means a
named dependency prevents validation.

## Evidence reviewed

| Source/symbol | Verified fact |
| --- | --- |
| [`SessionStore::persist`](../../../../../crates/cosh-core/src/session/store.rs#L125) | Uses validation, locking, generation conflict detection, redaction, bounds, and atomic file commit for one provider-session aggregate |
| [`ScopedStorage`](../../../../../crates/cosh-core/src/session/scoped.rs#L27) | Uses private permissions, descriptor-relative operations, no-follow opens, and temporary-file cleanup |
| [`CoshCoreService::new`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service.rs#L106) | Shell starts a worker that owns persistent cosh-core process state |
| [`service_loop`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service.rs#L283) | Shell resets or shuts down its core child based on per-turn state |
| [`spawn_provider_child`](../../../../../crates/cosh-shell/src/adapter/process.rs#L66) | Provider process gets a new session, piped I/O, and bounded retry |
| [`run_provider_process_loop`](../../../../../crates/cosh-shell/src/adapter/process.rs#L190) | Watchdog, bounded stderr, cancellation escalation, and reap exist in Shell |
| [`output_with_timeout`](../../../../../crates/cosh-core/src/process.rs#L72) | Core helper subprocess cleanup covers timeouts and caller cancellation |
| [`Cargo.toml`](../../../../../Cargo.toml) | No SQLite dependency is declared |

No real-provider, ECS, or host-privileged test is claimed. The commands above
are local targeted tests for the first implementation slice; the destructive
systemd containment fixture is recorded separately below. The historical
baseline itself remains documentation evidence.

## Acceptance matrix

| ID | Requirement | Baseline | Evidence required to pass |
| --- | --- | --- | --- |
| SS-01 | ADR-S1 explicitly accepts SQLite WAL, one writer, local filesystem only | PASS | Connection-policy and private-path tests. |
| SS-02 | Task event, projection, idempotency, and Outbox commit atomically | PASS | Duplicate Delivery ID and exact-bound overflow fixtures roll the projection/event/receipt/Outbox transaction back without mutation; the raw writer is crate-private in release builds. |
| SS-03 | Schema migrations are checksummed, fail closed, backed up, and restorable | PARTIAL | Online backup/restore verifies read-only preflight, source identity, no-clobber, private output, durability, and round trip; the complete operator runbook and kill/power-loss matrix remain. |
| SS-04 | Private path, no-follow, ownership, and file-type checks protect all SQLite companion files | PARTIAL | Absolute/private/path-component tests pass; race-free descriptor-relative open and ownership checks remain. |
| SS-05 | Event revisions and identity parents are enforced by database constraints | PARTIAL | Strict DDL enforces event ID, `(task_id, revision)`, and available foreign keys; not every parent is a DB row yet. |
| SS-06 | Unknown execution outcome never auto-replays unsafe side effects | PARTIAL | Task/Run recovery fails closed and does not auto-retry an unproven effect; production ExecutionTarget reconciliation is future optional capability work. |
| SS-07 | ADR-S2 gives one `RuntimeSupervisor` all Agent child ownership | PARTIAL | Supervisor first slice and owned tests are separately verified; daemon ownership migration remains. |
| SS-08 | Shell owns native PTY only after migration; bridges own no process handles | Missing | Ownership inventory and compile/API review |
| SS-09 | Every spawn has process-group cleanup, bounded I/O, reap, and generation fencing | PARTIAL | Supervisor cleanup, bounded I/O, generation fencing, and packaged-unit cgroup containment pass for the Gateway slice; Shell-owned compatibility paths remain. |
| SS-10 | Restart backoff and circuit-open health prevent crash loops | Missing | Deterministic clock/restart-budget tests |
| SS-11 | Daemon restart fences bindings, reclaims leases, and reconciles executions | PARTIAL | Scheduler lease reclaim, Runtime-bound fail-closed convergence, durable input dispatch replay, and a real-`SIGKILL` local SQLite restart fixture exist; production side-effect reconciliation, exact-candidate power-loss, and universal reconciliation remain future work. |
| SS-12 | Session, audit, evidence, and Task stores remain separate | PASS | New Gateway schema does not replace SessionStore/audit/evidence. |
| SS-13 | Bilingual documents, links, and commands are equivalent | PASS | Reciprocal links and implementation evidence are mirrored. |
| SS-14 | An external lifecycle owner contains Runtime descendants after Gateway `SIGKILL` | PARTIAL | The destructive fixture reported PASS in a disposable Ubuntu 24.04 arm64 container with systemd 255. It verified the rendered packaged unit, direct child/grandchild/double-fork/`setsid` cleanup, failed transient-unit escape, and replacement readiness after the old cgroup emptied. An exact candidate commit and other supported production environments remain unverified. |

`PARTIAL` records a verified slice, not complete supervisor or storage exit.

## Required fixtures and artifacts

```text
fixtures/gateway-storage/v1/
  schema.sql
  migrations/
    0001_initial.sql
  task-command-atomicity.json
  outbox-reclaim.json
  execution-outcome-unknown.json
  migration-checksums.json
  corrupt/
    newer-schema.db
    invalid-foreign-key.db
    truncated-wal.db
fixtures/runtime-supervisor/v1/
  fake-core-normal
  fake-acp-normal
  malformed-initialize
  oversized-line
  stderr-flood
  close-stdout
  ignore-term
  spawn-grandchild
  crash-loop
```

Required operational artifacts:

- accepted ADR-S1 and ADR-S2;
- schema diagram and migration compatibility table;
- state-path and file-permission specification;
- backup/restore runbook with verification result;
- disk-full, corruption, stuck WAL, and crash-loop runbooks;
- process ownership inventory proving every child has one owner;
- supervisor transition and shutdown traces from deterministic fixtures.

These artifacts are absent on the baseline.

## Required validation commands

Final package names may follow the implementation scaffold, but acceptance
must record equivalent targeted commands and exact counts:

```bash
cargo test --package cosh-gateway storage
cargo test --package cosh-gateway --test storage_faults
cargo test --package cosh-gateway runtime_supervisor
cargo test --package cosh-gateway --test supervisor_process_tree -- --test-threads=1
cargo test --package cosh-shell --test protocol
cargo test --package cosh-shell --test shell_host -- --test-threads=4
```

The Shell targets validate that migration did not regress current protocol and
PTY ownership. They are future implementation gates, not commands run for this
documentation change.

## Mandatory failure scenarios

| Scenario | Required outcome |
| --- | --- |
| Crash before Task transaction commit | No event, projection, or Outbox partial state |
| Crash after commit before dispatch | Outbox replays with the same Delivery ID |
| Crash after Permit consume before result | Execution becomes `outcome_unknown`; no unsafe automatic replay |
| Database newer than binary | Startup fails without mutation |
| Migration checksum mismatch | Startup fails and preserves backup/source database |
| WAL or disk full | The `SQLITE_FULL` fixture proves transaction rollback without false success; the complete host disk-health gate remains open |
| Runtime emits after replacement | Generation fence rejects Task mutation |
| Child ignores protocol cancel and TERM | Process group receives KILL and all descendants are reaped |
| Child floods stderr or sends huge frame | Memory remains bounded; Runtime fails with a safe code |
| Daemon shuts down with active Task | Durable state remains explainable; no orphan Agent Runtime child |
| Gateway receives `SIGKILL` with active descendants | External owner kills the complete cgroup; replacement readiness waits for it to empty; started effects become uncertain and never auto-replay |

## Remaining implementation

- No-clobber online backup/restore and redacted read-only inspection exist;
  checkpoint/disk health, corruption quarantine, the operator procedure, and
  complete power-loss evidence remain.
- Outbox lease/dispatch/ack, Run lease, Runtime binding, and task/input
  recovery exist; production ExecutionTarget uncertainty reconciliation and
  complete hard-crash kill-point coverage remain future work.
- Current path checks are fail-closed but are not yet descriptor-relative and race-free across open.
- `RuntimeSupervisor` owned tests, Gateway daemon scheduling, and generation
  fencing exist; restart backoff/circuit-open behavior and Shell ownership
  migration remain.
- cosh-core and provider child ownership is still Shell-local for interactive
  use.
- ADR-S3 now has an opaque verifier, packaged systemd unit, and a destructive
  hard-`SIGKILL` process-tree PASS on Ubuntu 24.04 arm64/systemd 255. Evidence
  is not yet tied to an exact candidate commit or repeated across supported
  production environments.
- Installed ACP and the contained task-only brokered Core profile run through
  the daemon scheduler. No production checkpoint/ws-ckpt path is enabled. Real
  Codex/Claude and manual Terminal gates remain unaccepted, and interactive
  Shell ownership has not migrated.

## Exit criteria

G0/implementation acceptance requires:

1. SS-01 through SS-14 pass on an exact recorded commit.
2. ADR-S1/S2 and remaining schema-affecting decisions are approved.
3. Every mandatory failure scenario has automated evidence.
4. Backup restoration is tested against the exact migration set.
5. Process-tree tests prove no direct child, grandchild, reader, or writer task
   leaks after cancellation and shutdown.
6. Restart recovery produces deterministic Task, Outbox, Runtime binding, and
   uncertain Execution states.
7. Existing SessionStore and audit fixtures remain green and unmigrated.
8. No privileged OS mutation, real provider, or ECS result is claimed unless
   separately requested and recorded.

## Validation record

- Reciprocal English/Chinese links are present.
- ADR decisions, schema draft, failure matrix, commands, and fixtures align
  across languages.
- Relative links resolve from this module directory.
- Markdown whitespace and diff hygiene were checked.
- Targeted storage, Task, and Runtime tests are recorded above. The destructive
  containment fixture was run only in the stated disposable systemd container.
  Real provider, manual Terminal, and ECS validation remain unaccepted.
