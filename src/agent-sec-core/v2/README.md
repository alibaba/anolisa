# AgentSecCore V2 Policy and daemon foundations

This workspace slice contains the dependency-light contracts, Policy
Administration Point, protocol-independent Unix-domain-socket service framework,
runnable foreground process bootstrap, the first AgentSight file-deletion
target Adapter, and its independent deployment Client used by later AgentSecCore
V2 work packages. It deliberately contains no daemon wire protocol, concrete
persistence or Policy compiler, Policy runtime, reconciliation worker, or outbox.

The current crates are:

- `asc-foundation-types`: bounded transport-independent identifiers and revisions.
- `asc-policy-types`: authored Policy and immutable prepared Policy/Scope/Binding
  snapshots, backend-independent IR, and target Adapter contracts.
- `asc-policy-adapter-agentsight`: deterministic file-deletion and PID-Scope
  translation into a compiler-checked AgentSight/ActPlane plan.
- `asc-agentsight-client`: health-gated AgentSight apply/delete transport for
  one configured endpoint, with process identity resolution and complete HTTP
  fixtures. It does not depend on a reconciliation framework.
- `asc-pap`: transport-independent current-record Policy/Scope/Binding CRUD with
  monotonic revisions over explicit compiler and repository ports.
- `asc-daemon-service`: bounded UDS admission, one-request framing, kernel peer
  credentials, dispatcher/rejection-encoder injection, connection isolation,
  dispatch cancellation, and controlled drain.
- `asc-daemon`: foreground process/bootstrap that installs Unix signal handling,
  selects explicit transport limits, binds an explicitly supplied socket, and
  runs `asc-daemon-service`.

## Daemon service boundary

`asc-daemon-service` is a `PARTIAL_MIGRATION` work package. It preserves the V1
one-request-per-connection LF/EOF framing, bounded first-frame read, bounded
connection admission, and socket ownership cleanup. Normal response encoding
belongs to an injected dispatcher; transport rejection encoding belongs to a
separate protocol-only port. Its acceptance type is current-version contract
testing with socket bytes and fake handlers. The V1 Python daemon is discovery
evidence only and is not linked or executed by the Rust runtime.

The service framework does not deserialize a daemon request, generate protocol
request IDs, choose authorization roles, or render protocol errors. The concrete
dispatcher receives a bounded raw request frame and owns method routing. Method
allowlist routing is internal to that one dispatcher implementation; it is not a
second service dispatch layer. A separate `RejectionEncoder` receives typed
transport failures and must remain independent of PAP/Repository state.

The current bootstrap bounds frame read, application dispatch, rejection
encoding, response write, connection drain, and final Tokio runtime shutdown.
Dispatch timeout releases transport capacity and signals cooperative cancellation;
it cannot forcibly stop an application blocking call that ignores that signal.
The framework also cannot prove that a concrete PAP/Repository avoids global
locks; that remains a required direct-consumer concurrency test at integration.

The current `asc-daemon` executable deliberately registers no wire methods. It
can start and exercise the real UDS lifecycle, but it closes complete requests
without a response until the daemon protocol is merged. Socket presence therefore
does not mean application readiness. It also requires an explicit absolute socket
path because packaging-owned system paths, singleton/stale-socket policy, runtime
directory hardening, and readiness remain later process-integration work.

Run the independent transport process in the foreground:

```bash
cargo run -p asc-daemon -- serve --socket /absolute/existing-directory/daemon.sock
```

After protocol integration, the existing daemon handler should implement
`RequestDispatcher` directly and be injected by this bootstrap. A small
protocol-only error encoder implements `RejectionEncoder`. PAP becomes one
registered method family inside the dispatcher; the service framework and
rejection path remain independent of PAP, its compiler, and its repository.

## Current-record revision boundary

Policy, Scope, and Binding each retain one current record per stable identity.
Changed writes advance a positive, never-reused revision and atomically replace
the previous current content. An exact GET for an older revision returns
not-found, and LIST returns at most one current record per identity.

Deleting current Policy or Scope content retains its allocation head as a
tombstone, so a later update of the same identity advances rather than reuses a
revision. A `PreparedBinding` embeds complete Policy and Scope snapshots; an
existing Binding therefore remains deterministic after either source record is
updated or deleted. A new Binding can select only a currently retained source
revision. PAP does not expose historical resource-version CRUD; durable
operation/audit history belongs to later work packages.

## Binding spec and lifecycle boundary

`PreparedBinding` is an immutable snapshot. The pair
`(binding_id, binding_revision)` identifies exactly one complete Policy/Scope
snapshot and must never be reused for a different desired-state operation.
Only the current Binding snapshot is retained. Mutable status is deliberately
outside that spec:

- `BindingStatus` contains only the lifecycle state; it carries no duplicated
  Binding ID or revision.
- `BindingView { spec, status }` joins one immutable spec with its status for
  GET/LIST responses.
- `bindingRevision` advances for every accepted, non-idempotent Apply or Delete
  intent, including reapplying identical content after failure or deletion.
- Reconciler claim, retry, completion, and failure transitions do not advance
  the revision.

All legal lifecycle states are shared in `asc-policy-types::binding`, next to
`PreparedBinding`, so PAP, the future outbox, and the future reconciler use one
contract:

| State | Meaning | Written by | Terminal without a new request? |
|---|---|---|---|
| `PENDING_APPLY` | Apply request accepted but not claimed | PAP | no |
| `APPLYING` | Apply work claimed and running | reconciler | no |
| `READY` | referenced spec applied successfully | reconciler | yes, success |
| `APPLY_FAILED` | Apply permanently failed or exhausted retries | reconciler | yes, failure |
| `PENDING_DELETE` | Delete request accepted but not claimed | PAP | no |
| `DELETING` | detach work claimed and running | reconciler | no |
| `DELETED` | detach completed successfully | reconciler | yes, success |
| `DELETE_FAILED` | detach permanently failed or exhausted retries | reconciler | yes, failure |

“Terminal” means that no automatic transition remains. A later user request can
still move lifecycle from a terminal state to a new pending state.

The successful creation path is:

```text
none --CREATE--> PENDING_APPLY --claim--> APPLYING --success--> READY
```

The successful deletion path is:

```text
apply-side state --DELETE--> PENDING_DELETE --claim--> DELETING --success--> DELETED
```

The complete legal transition set is:

| Current | Event | Next | Revision rule |
|---|---|---|---|
| none | CREATE valid spec | `PENDING_APPLY` | allocate revision 1 |
| `PENDING_APPLY`, `APPLYING`, `READY` | UPDATE identical spec | no-op | unchanged |
| `APPLYING`, `DELETING` | UPDATE changed spec | `OPERATION_IN_PROGRESS` | unchanged |
| `DELETING` | UPDATE identical spec | `OPERATION_IN_PROGRESS` | unchanged |
| any other state | UPDATE accepted Apply intent | `PENDING_APPLY` | allocate next revision and replace current record |
| `APPLYING` | DELETE | `OPERATION_IN_PROGRESS` | unchanged |
| `PENDING_APPLY`, `READY`, `APPLY_FAILED`, `DELETE_FAILED` | DELETE | `PENDING_DELETE` | allocate next revision and replace current record |
| `PENDING_DELETE`, `DELETING`, `DELETED` | DELETE | no-op | unchanged |
| `PENDING_APPLY` | worker claim | `APPLYING` | unchanged |
| `APPLYING` | success | `READY` | unchanged |
| `APPLYING` | retryable failure | `PENDING_APPLY` | unchanged |
| `APPLYING` | permanent/retry-exhausted failure | `APPLY_FAILED` | unchanged |
| `PENDING_DELETE` | worker claim | `DELETING` | unchanged |
| `DELETING` | success | `DELETED` | unchanged |
| `DELETING` | retryable failure | `PENDING_DELETE` | unchanged |
| `DELETING` | permanent/retry-exhausted failure | `DELETE_FAILED` | unchanged |

There are no other legal transitions. In particular, a user request may reverse
`PENDING_APPLY` or `PENDING_DELETE` because target-side work has not been
claimed, but it cannot interrupt `APPLYING` or `DELETING`. There is no
`APPLYING -> PENDING_DELETE` or `DELETING -> PENDING_APPLY` transition within
one revision.

Repositories atomically replace the single current Binding snapshot and status
when PAP accepts a new desired-state revision; no older Binding record remains.
A status-only worker transition does not rewrite spec content. Status CAS APIs
identify the current target by `binding_id` plus the revision contained in the
Binding and require the expected current status. Repository implementations
must repeat the `APPLYING`/`DELETING` admission gate inside the atomic update so
a worker claim cannot race a PAP pre-check.

The shared state machine is defined and tested now, but the PAP-only phase
implements no outbox, dispatcher, or reconciler. Therefore
PAP writes only `PENDING_APPLY` and `PENDING_DELETE`; nothing in this phase
advances them. TODO(policy-reconciliation): persist each accepted current
Binding replacement and its reconcile intent atomically, then let the future
Reconciler consume one complete `BindingView` whose embedded revision fences
claim, retry, completion, failure, restart recovery, and cancellation.

Daemon protocol, daemon client, concrete persistence/compiler, Policy runtime,
reconciliation worker, and outbox belong to later work packages and are
intentionally absent from this slice.

Run the branch-owned validation from this directory:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```
