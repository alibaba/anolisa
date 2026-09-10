# Binding reconciliation core

`asc-pcp` implements one synchronous `BindingReconciler::reconcile(binding_id)`
attempt. Async event delivery and timer ownership are deliberately outside this
crate. There is no PAP request handler, queue, daemon lifecycle or SQL backend.

## Boundaries

The Adapter/Client traits are defined in
[`asc-policy-target-contracts`](../asc-policy-target-contracts/README.md), with
their data in `asc-policy-types::target`. Both this core and concrete Clients
depend on that shared layer. Re-exports preserve existing core imports; Clients
do not depend on `asc-pcp`. The actual-PEP composition tests belong to this core's
dev-dependencies, not the standalone Client.

- `TargetBindingAdapter`: complete `PreparedBinding` to the existing opaque
  `TargetBindingPlan`. A closure can bridge the existing AgentSight Adapter
  without changing that component's API.
- `TargetDeploymentClient`: side-effect-free `prepare_apply`, then
  `create` / `update` / `delete`. Owns all target identities, cleanup bytes,
  replay validation and partial result classification. The core never interprets
  UUIDs, HTTP, DSL, PIDs or request bytes. Credentials do not belong in snapshots.
- `BindingStateRepository`: `get_binding_state` reads a complete aggregate;
  `compare_exchange_binding_state` atomically compares that snapshot and writes
  its replacement or removal (`next: None`). Defined with the storage data in
  [`asc-policy-repository`](../asc-policy-repository/README.md). The memory adapter
  uses PAP's authoritative Binding map and has no dependency on `asc-pcp`.
- `ReconcileExecution`: core-owned shared execution slots and pending outcomes.
  Supply the same `Arc<ReconcileExecution>` as the final constructor argument to
  every worker using a store. Slots and their mutexes are private to the core;
  callers cannot inspect or acquire execution ownership. `claim`, `register`,
  observation merging, lifecycle decisions and retry calculation are private
  core operations over read/CAS.
- `Clock`: monotonic milliseconds in the current process. The first claim saves
  the configured retry policy for that intent. A fresh pending record with
  `next_attempt_at = None` is immediately eligible; retries have an explicit
  deadline. Persisted wall-clock recovery is not implemented.

`update(previous, prepared)` excludes the current target identity from
`previous`. The saved `is_update` decision survives partial cleanup, so a retry
still calls `update` even when all old targets are already absent. A Client that
updates in place may receive an empty `previous` slice.

## Caller obligations

Call only after the intent has committed. Repeated notifications are safe:
the core reloads current state and enforces due time and attempt budget.

| Result | Caller action |
|---|---|
| `Completed`, `Skipped`, `Failed` | Do not reset the operation's budget; a later accepted request can notify again |
| `RetryAt { at }` | Arrange a timer; do not sleep while holding the Binding slot |
| `Superseded` | Hand off/recheck the latest intent, including Delete with the same revision |
| `StoreError` | Report storage health failure and retry bookkeeping with bounded caller scheduling; never infer remote absence |

An unfinished result transaction is retained in the core's shared execution slot.
The next call retries that transaction **before** reading/claiming new work and
does not repeat the completed Client request. The slot remains shared across
Reconciler instances. On a registration failure, no modifying call is made;
the same mechanism later returns that claimed attempt to retry/failure.

All Client calls are synchronous and must have concrete transport timeouts.
If called via `spawn_blocking`, retain and join the handle: timing out/aborting
the async waiter does not stop an already running blocking call.

An unwinding panic is caught while the execution guard remains owned. The core
tries to commit an already available outcome; otherwise it fails the claimed
operation with `RECONCILE_WORKER_PANICKED`, preserving unknown targets, prepared
bytes and cleanup responsibility. Revision/status CAS protects newer intent.
Failed bookkeeping stays cached, and retries perform no target I/O. The memory
repository retains the latest CAS write receipt. The core caches the exact write
ID and replacement before calling storage, so replay after a post-commit panic
also handles already-removed Absent records. For whole-Binding removal, absence
acknowledges replay; no receipt or tombstone remains after deletion.
A conflict causes a fresh read and
recalculation; unrelated concurrent changes are preserved. Registration and
completion cap CAS contention retries at 16 per call.

The core releases the healthy execution guard before resuming the original panic.
The caller must observe the failed join and project service health; it must not
log the panic payload as a public error. An independently poisoned repository or
slot remains unavailable until repaired. This mechanism requires unwinding and
atomic repository transactions; it cannot recover a process abort or make a
corrupted backend usable. Process restart recovery still requires durable state.

Absent Binding IDs allocate no slot. Existing Bindings retain the same slot,
including Ready/failed states. After physical removal and acknowledgement of all
pending bookkeeping, the core removes that exact slot from the registry. Existing
holders/waiters keep their Arc, re-read absence and perform no target I/O. IDs
cannot be reused; a stale allocator also re-reads absence and retires its slot.
This cannot split execution ownership for any live Binding.

## Current integration status

PAP now uses spec-only revisions, irreversible Delete and conditional updates.
`pap_lifecycle.rs` composes the real PAP service, memory repository and core with
a scripted Client: same-spec retry preserves requests; Delete retries retain
cleanup targets; successful cleanup removes the aggregate; re-deployment creates
a new ID. Race fixtures separately inject snapshots through raw CAS to exercise
core fencing, including synthetic delayed completion; they do not define PAP
admission. No reconciliation admission method lives in the memory adapter.
The daemon notification/timer worker is still not connected to this core.

The actual AgentSight Adapter is tested through the core port. The actual
[AgentSight Client](../../integrations/asc-agentsight-client/README.md) now
implements `TargetDeploymentClient`; a composition root can inject it directly.
Its preparation/replay and update/partial-result behavior is verified with the
actual Adapter, Client, Ureq transport and memory repository against a loopback
HTTP mock. Core isolation tests still use scripted Clients; no AgentSight
implementation is hidden inside the Reconciler.

## Validation

From `v2`:

```sh
cargo test -p asc-pcp --locked --offline -- --nocapture
cargo test -p asc-pcp --test agentsight_integration --locked --offline
```

Core fixture and panic-recovery checks live in `src/acceptance_tests.rs` and
`src/panic_recovery_tests.rs` as unit tests, so lock-lifetime, slot-retirement and
unknown-ID allocation assertions do not require public synchronization APIs.
The real Adapter/Client and PAP composition checks remain integration tests.

The [acceptance standard](../../../fixtures/reconciliation/ACCEPTANCE.md) and
[execution report](../../../fixtures/reconciliation/RESULTS.md) distinguish
core and real-Client composition evidence from the still-pending daemon worker slice.

Storage is memory-only: process exit loses intents, prepared requests, cached
outcomes and cleanup records. No cross-restart recovery, real PEP enforcement
or kernel behavior is established by these tests.

Before daemon wiring, add DJOB/DPROC fixtures for failed-join health, owned-task
shutdown and stopping new mutations on storage failure. Durable startup recovery
must use domain transactions/idempotency plus singleton ownership; a process-local
mutex alone does not establish cross-process safety. The current core is not wired
to daemon requests and does not claim these integration gates.
