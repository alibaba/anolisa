# Phase 1 Task Execution Plane Acceptance Baseline

[中文版](acceptance_zh.md) | [Design](design.md)

## Baseline result

**Overall: NOT IMPLEMENTED at `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`.** The repository
has robust provider-session persistence and audit evidence, but neither is a durable Task
aggregate. There is no coordinator, Task event store, Run lease, idempotency ledger, or outbox.

This is a readiness report, not evidence that Phase 1 behavior passed.

## First implementation result

**Overall: RUNNABLE DURABLE SLICE; PHASE 1 EXIT NOT ACCEPTED.** The current working-tree candidate
adds shared Task IDs/events, a deterministic reducer, an atomic SQLite Task store, the sole-writer
`TaskCoordinator`, fenced Run leases, Outbox scheduling, durable Runtime bindings, approval
resolution, and execution settlement. Universal governed execution and complete crash/kill-point
evidence remain open.

Current-worktree evidence is exercised with these scoped commands:

- `cargo test --locked --package cosh-gateway task::aggregate --no-fail-fast`.
- `cargo test --locked --package cosh-gateway storage --no-fail-fast`.
- `cargo clippy --locked --package cosh-gateway --lib -- -D warnings` passed.
- Tests cover revision gaps without mutation, explicit approval waiting, denial suspension, Run and
  Task terminal closure, in-memory schema-version rejection, actor substitution, actor-scoped
  idempotency replay/conflict, stale revisions, atomic Outbox rollback, schema/checksum rejection,
  private-path attacks, causation persistence, and event replay after a durable reopen.

## Durable ledger slice

**Overall: VERIFIED STORAGE SLICE; PHASE 1 EXIT NOT ACCEPTED.** The candidate now uses checksummed
schema v9 for durable approval/input state, runtime binding, Run leases, Outbox dispatch, and typed
Runtime input request/dispatch records. Generic permit/execution ledger fields remain reusable
foundations, but no production `ExecutionTarget`, checkpoint loop, or ws-ckpt dependency is wired.
Every Task/ledger mutation replays the authoritative Task event stream before using a Task/Run
binding. Runtime event acceptance requires the exact current, unexpired lease generation and
revision. `task.retry` creates a new fenced Run only after the previous Run is quiescent. An exact
pending Runtime input request can move the active Run to waiting and a single matching response
moves it back to running; raw input responses exist only in the private dispatch row, while Task
events and receipts retain a digest.

Focused evidence is reproduced with:

- `cargo test --locked --package cosh-gateway storage --no-fail-fast`.
- Adversarial fixtures cover a valid Run from another Task, stale lease revision, skipped Runtime
  generation, permit expiry wider than approval, cross-plane idempotency-key reuse, SQLite integer
  overflow, terminal receipt divergence, and rollback of rejected permit/execution mutations.
- The task-command and ledger-command receipt tables enforce one actor-scoped idempotency
  namespace. Earlier migration checksums remain unchanged and an existing v1 store upgrades through v9.

The daemon now wires `TaskCoordinator`, Outbox lease/reclaim/ack, Runtime dispatch, and task/input
recovery. No checkpoint executor, ws-ckpt target, or production side-effect result loop is enabled.
The implementation is not a universal executor or reconciliation service for Shell, Skill, MCP,
extension tools, or legacy mutation paths.

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
| TEP-002 | Coordinator is the only aggregate writer. | PARTIAL | Daemon commands and scheduler settlement use `TaskCoordinator`; a final ownership audit across future adapters remains. |
| TEP-003 | State reducer rejects every illegal transition. | PASS for current schema | The exhaustive 21-event by 9-state reducer matrix covers legal and illegal transitions, including exact pending input and retry, and verifies that rejection does not mutate the aggregate. |
| TEP-004 | Event, snapshot, idempotency receipt, and outbox commit atomically. | PASS | `commit_task` uses `BEGIN IMMEDIATE`; a duplicate Delivery ID proves complete rollback. |
| TEP-005 | Expected revision prevents stale writers. | PASS | Revision-conflict test leaves all Task tables empty. |
| TEP-006 | Run lease has monotonic fencing and bounded renewal. | PASS | Lease acquire/renew/release requires exact owner, revision, generation, active Task/Run, and deadline; takeover increments generation. |
| TEP-007 | Lease expiry never replays an unknown OS effect automatically. | PARTIAL | Task/Run recovery fails closed and does not auto-retry an unproven effect; typed side-effect reconciliation is future optional capability work. |
| TEP-008 | Approval resolution is first-valid-terminal-wins. | PARTIAL | Durable pending-state CAS and asynchronous API resolution exist; approval-to-permit and production target integration remain future work. |
| TEP-009 | Runtime and execution callbacks are idempotent and fenced. | PARTIAL | Current lease/generation/monotonic-sequence fences, durable dispatch receipts, and source validation cover Task/Runtime callbacks; production execution-result replay remains future work. |
| TEP-010 | Event replay rebuilds an equivalent projection. | PASS | Durable-reopen recovery replays ordered envelopes and compares the exact snapshot. |
| TEP-011 | Outbox restart is at-least-once with stable Delivery IDs. | PARTIAL | Dispatch leasing, reclaim, and stable Delivery IDs are retained for the task-only slice; no checkpoint-specific acknowledgement/result loop is claimed. Universal delivery, remote paths, and power-loss evidence remain outside this result. |
| TEP-012 | Task records exclude raw streams, secrets, and terminal buffers. | PASS for current Task/store surface | The release public surface cannot call the raw writer. Task and Outbox payloads are limited to 256 KiB each and a commit to 1 MiB before any transaction. Raw input responses exist only in the private typed dispatch row; Task events and receipts contain a digest. Exact-boundary and overflow tests prove zero mutation on rejection. |
| TEP-013 | Corrupt/incompatible histories fail closed and remain inspectable. | PARTIAL | Schema/replay fails closed; redacted read-only `admin inspect` covers newer schema, checksum, foreign-key, and truncated databases and proves bytes are unchanged. Automatic quarantine remains. |
| TEP-014 | Provider `SessionStore` remains separate from Task storage. | PASS | Gateway SQLite is a separate crate/store and schema. |
| TEP-015 | Final storage engine and durability profile are approved. | PASS for scope decision | ADR-S1 accepts SQLite WAL with `synchronous=FULL`, one writer, and local filesystems. Exact-candidate power-loss and operator evidence remains a separate Phase 0/exit gate. |

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

1. TEP-001 through TEP-014 are PASS; TEP-015 is no longer a decision blocker, while its named
   exact-candidate durability gates still must pass.
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
- Treating a process PID or lease timeout as completion can repeat side effects.
- Letting presenters or callbacks mutate approval state creates split-brain authorization.
- No production execution proof is claimed in this task-only baseline. Treating generic ledger
  foundations as a universal Broker would leave Shell, Skill, MCP, extension, and legacy effects
  outside policy.
