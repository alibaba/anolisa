# Binding aggregate repository contract

This crate defines shared storage data and two database operations, below both
`asc-pcp` and `asc-pap-repository-memory`. It contains no worker, execution locks,
retry calculation, Client calls or lifecycle transition decisions.

| Operation | Contract |
|---|---|
| `get_binding_state(id)` | Consistent optional snapshot of the authoritative Binding, runtime and deployments |
| `compare_exchange_binding_state(expected, write)` | Compare the complete snapshot and atomically replace or remove the aggregate; return `Applied`, `AlreadyApplied` or `Conflict` |

`BindingStateWrite` carries a fresh write ID and `next: Option<BindingStateSnapshot>`.
`Some` replaces an existing aggregate; `None` removes the Binding, runtime,
deployments and receipt atomically. A replacement never inserts an absent ID.
A repeated removal of an absent ID returns `AlreadyApplied`: Binding IDs are
server-generated and never reused, so removal needs no permanent tombstone.
CAS includes runtime and deployment data: matching public revision/status alone
cannot authorize overwriting newer runtime changes. Reconciler writes preserve
the Binding spec; request admission belongs to PAP. The memory implementation
shares PAP's current Binding map. A receipt copy is acknowledgement metadata,
never a second authoritative Binding.

For existing aggregates, the latest CAS receipt survives PAP writes. Replaying its identical write ID and
contents returns `AlreadyApplied` without changing current data; reusing that ID
with different contents is invalid. Callers must share Reconciler execution
ownership and acknowledge ambiguous results before starting another CAS writer
for the same Binding. This is a bounded latest-write receipt, not historical
idempotency for arbitrary out-of-order writers. Backend transaction errors commit nothing; an error/panic after a committed
transaction may make acknowledgement ambiguous. Replaying the retained write
acknowledges it without repeating target I/O. Unwind leaves a transaction either
fully committed or fully uncommitted.

Runtime data keeps its existing serialized shape. `asc-pcp::ReconcileRecord`
re-exports the aggregate as an alias for existing complete fixtures. The current
implementation is process-local memory; SQL persistence and restart recovery
remain separate work packages.
