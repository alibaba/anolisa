# Phase 1 Task Execution Plane Acceptance Baseline

[中文版](acceptance_zh.md) | [Design](design.md)

## Baseline result

**Overall: NOT IMPLEMENTED at `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`.** The repository
has robust provider-session persistence and audit evidence, but neither is a durable Task
aggregate. There is no coordinator, Task event store, Run lease, idempotency ledger, or outbox.

This is a readiness report, not evidence that Phase 1 behavior passed.

## First implementation result

**Overall: VERIFIED FIRST SLICE; PHASE 1 EXIT NOT ACCEPTED.** The current working-tree candidate
adds shared Task IDs/events, a deterministic reducer, and an atomic SQLite Task store. It does not
yet add the sole-writer `TaskCoordinator`, Run lease/fencing, Outbox delivery workers, or execution
reconciliation.

Evidence recorded on 2026-08-13:

- `cargo test --locked --package cosh-gateway task::aggregate --no-fail-fast` passed 6/6 tests.
- `cargo test --locked --package cosh-gateway storage --no-fail-fast` passed 14/14 tests.
- `cargo clippy --locked --package cosh-gateway --lib -- -D warnings` passed.
- Tests cover revision gaps without mutation, explicit approval waiting, denial suspension, Run and
  Task terminal closure, in-memory schema-version rejection, actor substitution, actor-scoped
  idempotency replay/conflict, stale revisions, atomic Outbox rollback, schema/checksum rejection,
  private-path attacks, causation persistence, and event replay after a durable reopen.

## Durable ledger slice

**Overall: VERIFIED STORAGE SLICE; PHASE 1 EXIT NOT ACCEPTED.** The candidate now adds a
checksummed v2 migration for durable approval, single-use permit, execution, runtime-binding, and
Run-lease records. Every ledger mutation replays the authoritative Task event stream before using
a Task/Run binding. Permit consumption and Runtime event acceptance require the exact current,
unexpired lease generation and revision. Execution start consumes its permit atomically, while
restart recovery marks an unreceipted started execution `uncertain` without retrying it.

Evidence recorded on 2026-08-13:

- `cargo +1.88.0 test --locked --package cosh-gateway storage --no-fail-fast` passed 28/28
  targeted storage tests.
- Adversarial fixtures cover a valid Run from another Task, stale lease revision, skipped Runtime
  generation, permit expiry wider than approval, cross-plane idempotency-key reuse, SQLite integer
  overflow, terminal receipt divergence, and rollback of rejected permit/execution mutations.
- The task-command and ledger-command receipt tables enforce one actor-scoped idempotency
  namespace. The v1 migration checksum remains unchanged and an existing v1 store upgrades to v2.

This slice does not add `TaskCoordinator`, Outbox delivery workers, an executor, reconciliation,
or a daemon API. Runtime sequence callbacks reject duplicate sequence numbers rather than replaying
a stored callback result.

## Result vocabulary

| Result | Meaning |
| --- | --- |
| PASS | The pinned source and a reproducible artifact satisfy the criterion. |
| FAIL | A present implementation violates the criterion. |
| PARTIAL | A production slice exists, but named proof or behavior remains incomplete. |
| NOT IMPLEMENTED | No production path exists for the criterion. |
| BLOCKED | A named upstream decision or dependency prevents verification. |

## Baseline evidence

- `git rev-parse HEAD` identified
  `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`.
- [`session.rs`](../../../../../crates/cosh-core/src/session.rs) defines provider-session schema,
  identity, generation, summary, and health.
- [`session/store.rs`](../../../../../crates/cosh-core/src/session/store.rs) atomically persists one
  provider session with optimistic generation.
- [`runtime/state.rs`](../../../../../crates/cosh-shell/src/runtime/state.rs) is Shell in-memory
  presentation/runtime state.
- [`audit/event.rs`](../../../../../crates/cosh-types/src/audit/event.rs) is security evidence and
  does not own Task transitions.
- Repository search found no `TaskCoordinator`, `TaskEventStore`, `TaskId`, or Task outbox.

## Acceptance matrix

| ID | Criterion | Baseline | Evidence or missing artifact |
| --- | --- | --- | --- |
| TEP-001 | Typed `TaskId`, `RunId`, and lifecycle schemas exist. | PASS | `cosh-gateway-contracts::{ids,task}`. |
| TEP-002 | Coordinator is the only aggregate writer. | NOT IMPLEMENTED | Coordinator absent. |
| TEP-003 | State reducer rejects every illegal transition. | PARTIAL | Reducer exists and critical transition tests pass; exhaustive state/event matrix is pending. |
| TEP-004 | Event, snapshot, idempotency receipt, and outbox commit atomically. | PASS | `commit_task` uses `BEGIN IMMEDIATE`; a duplicate Delivery ID proves complete rollback. |
| TEP-005 | Expected revision prevents stale writers. | PASS | Revision-conflict test leaves all Task tables empty. |
| TEP-006 | Run lease has monotonic fencing and bounded renewal. | PASS | Lease acquire/renew/release requires exact owner, revision, generation, active Task/Run, and deadline; takeover increments generation. |
| TEP-007 | Lease expiry never replays an unknown OS effect automatically. | PARTIAL | Started executions recover as `uncertain` and are not retried; executor reconciliation is absent. |
| TEP-008 | Approval resolution is first-valid-terminal-wins. | PARTIAL | Durable pending-state CAS binds actor, revision, deadline, Task, and Run; a concurrent-decision fixture and Task-event integration remain. |
| TEP-009 | Runtime and execution callbacks are idempotent and fenced. | PARTIAL | Ledger commands replay by actor/key/digest; permit start and Runtime sequence require the current lease. Runtime sequence replay and Runtime-port integration remain. |
| TEP-010 | Event replay rebuilds an equivalent projection. | PASS | Durable-reopen recovery replays ordered envelopes and compares the exact snapshot. |
| TEP-011 | Outbox restart is at-least-once with stable Delivery IDs. | PARTIAL | Stable rows persist, but dispatch leasing/reclaim/ack does not exist. |
| TEP-012 | Task records exclude raw streams, secrets, and terminal buffers. | PARTIAL | Snapshot/event leaves are typed and bounded; Outbox payload, collection aggregate bounds, and secret classification remain. |
| TEP-013 | Corrupt/incompatible histories fail closed and remain inspectable. | PARTIAL | Schema/replay fails closed; quarantine and inspect surface are pending. |
| TEP-014 | Provider `SessionStore` remains separate from Task storage. | PASS | Gateway SQLite is a separate crate/store and schema. |
| TEP-015 | Final storage engine and durability profile are approved. | BLOCKED | Phase 0 storage ADR is pending. |

## Required fixtures, commands, and artifacts

| Artifact | Required proof |
| --- | --- |
| `task-events-v1` golden corpus | Stable codecs, required/optional compatibility, bounds. |
| Complete transition table | Every state/command pair has an expected result. |
| `task-store-vN` migration fixtures | Upgrade, backup, inspect, and incompatible-version behavior. |
| Kill-point matrix | Atomicity before/during/after commit and delivery acknowledgment. |
| `expired-lease-uncertain-effect` | New worker suspends instead of re-executing. |
| Concurrent approval fixture | Exactly one conflicting terminal decision wins. |
| Replay digest artifact | Live projection equals event-reduced projection. |

Expected commands after implementation are:

```bash
cargo test --package cosh-gateway task_model
cargo test --package cosh-gateway task_store
cargo test --package cosh-gateway task_crash_recovery
cargo test --package cosh-gateway-contracts task_schema
```

The implemented target names are broader than the original placeholders. The exact targeted
commands and counts are recorded above. Full workspace gates and live/ECS validation remain outside
this scope-proportional first slice.

## Exit criteria

1. TEP-001 through TEP-014 are PASS and the Phase 0 decision clears TEP-015.
2. Model, concurrent-writer, crash, corruption, migration, and reconciliation fixtures pass at the
   exact candidate commit.
3. A code-ownership check proves adapters, handlers, bridges, workers, and presenters cannot write
   Task storage outside `TaskCoordinator`.
4. Security review verifies tenant/workspace scope, actor/delegation, event redaction, lease fence,
   approval races, uncertain execution, and store permissions.
5. The acceptance report lists the exact store engine/configuration, commands, test counts,
   artifacts, unsupported migration paths, and rollback procedure.

## Current risks

- Extending provider `SessionStore` would conflate model conversation with control-plane truth.
- A file-per-Task design may not support atomic event/idempotency/outbox commit without an
  additional transaction layer.
- Treating a process PID or lease timeout as completion can repeat side effects.
- Letting presenters or callbacks mutate approval state creates split-brain authorization.
