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

Recorded on 2026-08-13:

- `cargo test --locked --package cosh-gateway storage --no-fail-fast`: 14/14 passed.
- `cargo test --locked --package cosh-gateway task::aggregate --no-fail-fast`: 6/6 passed.
- `cargo clippy --locked --package cosh-gateway --lib -- -D warnings`: passed.
- Automated evidence covers WAL/FULL/foreign-key policy, actor and revision substitution, atomic
  Task/Event/receipt/Outbox rollback, checksummed and newer-schema failure, deterministic reopen
  recovery, causation rows, relative paths, insecure parents without chmod, and intermediate or
  final symlinks.

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

No provider, ECS, privileged, or live process tests were run. The commands above are local targeted
tests for the first implementation slice; the historical baseline itself remains documentation
evidence.

## Acceptance matrix

| ID | Requirement | Baseline | Evidence required to pass |
| --- | --- | --- | --- |
| SS-01 | ADR-S1 explicitly accepts SQLite WAL, one writer, local filesystem only | PASS | Connection-policy and private-path tests. |
| SS-02 | Task event, projection, idempotency, and Outbox commit atomically | PASS | Duplicate Delivery ID rolls the projection/event/receipt transaction back. |
| SS-03 | Schema migrations are checksummed, fail closed, backed up, and restorable | PARTIAL | Checksum/newer-schema/quick-check pass; online backup and restore fixture remain. |
| SS-04 | Private path, no-follow, ownership, and file-type checks protect all SQLite companion files | PARTIAL | Absolute/private/path-component tests pass; race-free descriptor-relative open and ownership checks remain. |
| SS-05 | Event revisions and identity parents are enforced by database constraints | PARTIAL | Strict DDL enforces event ID, `(task_id, revision)`, and available foreign keys; not every parent is a DB row yet. |
| SS-06 | Unknown execution outcome never auto-replays unsafe side effects | Missing | Crash-boundary reconciliation tests |
| SS-07 | ADR-S2 gives one `RuntimeSupervisor` all Agent child ownership | PARTIAL | Supervisor first slice and owned tests are separately verified; daemon ownership migration remains. |
| SS-08 | Shell owns native PTY only after migration; bridges own no process handles | Missing | Ownership inventory and compile/API review |
| SS-09 | Every spawn has process-group cleanup, bounded I/O, reap, and generation fencing | PARTIAL | Supervisor process cleanup and bounded I/O are separately tested; generation fencing remains. |
| SS-10 | Restart backoff and circuit-open health prevent crash loops | Missing | Deterministic clock/restart-budget tests |
| SS-11 | Daemon restart fences bindings, reclaims leases, and reconciles executions | Missing | End-to-end restart fixtures |
| SS-12 | Session, audit, evidence, and Task stores remain separate | PASS | New Gateway schema does not replace SessionStore/audit/evidence. |
| SS-13 | Bilingual documents, links, and commands are equivalent | PASS | Reciprocal links and implementation evidence are mirrored. |

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
| WAL or disk full | Admission stops with stable degraded health; no false success |
| Runtime emits after replacement | Generation fence rejects Task mutation |
| Child ignores protocol cancel and TERM | Process group receives KILL and all descendants are reaped |
| Child floods stderr or sends huge frame | Memory remains bounded; Runtime fails with a safe code |
| Daemon shuts down with active Task | Durable state remains explainable; no orphan Agent Runtime child |

## Remaining implementation

- No online backup/restore, checkpoint/disk health, corruption quarantine, or operator procedure.
- No Outbox lease/dispatch/ack loop, Run lease, or uncertain execution reconciliation.
- Current path checks are fail-closed but are not yet descriptor-relative and race-free across open.
- A `RuntimeSupervisor` first slice exists with 18 owned tests; Gateway daemon integration,
  restart policy, and generation fencing remain.
- cosh-core and provider child ownership is still Shell-local for interactive
  use.
- Library tests can launch a fake ACP child through an `AcpV1RuntimeBridge`
  that embeds the sole `RuntimeSupervisor`; no installed entrypoint, live
  adapter evidence, restart ownership, or daemon integration exists.

## Exit criteria

G0/implementation acceptance requires:

1. SS-01 through SS-13 pass on an exact recorded commit.
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
- Targeted storage, Task, and Runtime tests are recorded above. Full workspace,
  live-system, provider, privileged, and ECS validation was intentionally not run.
