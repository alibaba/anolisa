# AgentSecCore V2 PAP daemon slice

This workspace implements the first Rust-native Policy Administration Point
slice. It accepts product Policy Templates, lowers them to backend-independent
Canonical Policy IR, stores immutable Policy and Scope revisions, and accepts
durable Binding intents.

The implementation deliberately stops at the `PolicyAdapter` port:

```text
UDS DTO -> daemon-core -> PAP -> SQLite -> outbox -> PolicyAdapter
```

The internal reconcile state `DISPATCHED` means only that an Adapter accepted the target-independent
`AdapterCommand`. It does not mean a target policy was compiled, installed, or
made effective. The production composition currently uses
`UnavailablePolicyAdapter`, so internal work items stop at
`BLOCKED/adapter_unavailable`. No downstream service protocol, target mapping,
kernel identity, or deployment receipt is part of this workspace slice.

Policy and Scope writes complete lowering, validation and durable storage before
returning `STORED`. Binding writes atomically persist the desired revision,
internal reconcile work item, outbox record and audit record before returning
`ACCEPTED`; Adapter dispatch is asynchronous. A retryable Adapter result enters
the internal `RETRY_WAIT` state and is not automatically retried by a timer in
this phase, preventing a retry busy loop. A later Adapter work package owns the
explicit retry/reconcile trigger.

## Crates

- `asc-policy-types`: Canonical Policy IR contracts.
- `asc-policy-engine`: deterministic product Template lowering.
- `asc-pap`: immutable Policy/Scope authoring and repository ports.
- `asc-policy-runtime`: prepared Binding state, internal reconcile lifecycle,
  outbox and `PolicyAdapter` port.
- `asc-persistence-sqlite`: transactional PAP/runtime persistence.
- `asc-daemon-protocol`: shared daemon envelope plus capability-scoped strict
  Policy DTOs and method metadata.
- `asc-daemon-client`: bounded authenticated UDS client and W3C trace-context
  propagation; it never starts the daemon or reads daemon-owned state.
- `asc-daemon-core`: transport-independent capability services; the current
  implementation exports `policy::PolicyService` rather than a global god
  object.
- `asc-daemon`: UDS, credential verification, composition and background worker.
- `asc-cli`: daemon-only Rust command adapter, built as `asc-cli`; it exposes the
  current Policy Template, Scope and Binding methods without adding defaults or
  bypassing the daemon.

Every library root is a facade: `lib.rs` declares modules and exposes the stable
public surface, while models, errors, ports, services and adapters live in
ownership-specific modules. Cross-capability composition belongs in the
`asc-daemon` application state. Registered methods carry explicit capability
and access metadata; security decisions must not be inferred from a string
prefix.

## Local development

Generate the management credential as a one-time preparation step:

```bash
cargo run -p asc-daemon -- prepare-auth --token-file /secure/path/policy-admin.token
```

Start the daemon only after its socket and database parent directories have
been provisioned:

```bash
cargo run -p asc-daemon -- serve \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --database /var/lib/agent-sec-core/v2/policy.db \
  --token-file /run/agent-sec-core/v2/policy-admin.token
```

V2 intentionally uses a separate runtime namespace while the shipped Python V1
CLI and daemon remain supported. `AGENT_SEC_DAEMON_SOCKET` and the V1 per-user
`XDG_RUNTIME_DIR` socket convention belong to V1 and are not interpreted by
this Rust slice. `asc-cli` requires explicit `--socket` and `--token-file`
arguments; a future installation migration must deliberately define whether
those V1 contracts are superseded.

The daemon handles `SIGTERM` and `SIGINT` by stopping its accept loop. Its
socket owner guard removes the path only when it still names the same socket
device/inode that this process bound, so cleanup cannot unlink a replacement
created by another process. Uncatchable termination such as `SIGKILL` cannot run
process cleanup; safe stale-socket recovery after such a crash remains part of
the later host-singleton work package.

Build and use the Rust CLI after the daemon is ready:

```bash
cargo build -p asc-cli --bin asc-cli
```

`policy-template.json` contains the exact daemon `PutPolicyParams` object. PUT
declares the complete desired template: the caller does not choose a revision,
and the CLI does not add defaults or perform lowering:

```json
{
  "policyName": "protect-production-secrets",
  "template": {
    "kind": "high_sensitivity_read_deny",
    "files": ["/secrets/**"]
  }
}
```

Omitting `policyId` creates a new Policy with a daemon-generated UUID and
revision 1. Supplying the returned UUID updates only that existing Policy; an
unknown UUID returns `not_found`. `policyName` is a required, non-unique user
name and is never used as a reference key. Repeating an update with the exact
active name and template returns its existing revision. Changing the name or
template creates the next daemon-assigned revision while older revisions remain
queryable. Repeating an ID-less create intentionally creates another Policy.

Put and query resources through the daemon:

```bash
target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy template put --file ./policy-template.json

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy template get <policyId-from-put-response> --revision 1

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy template delete <policyId-from-put-response> --revision 1

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy scope put --pid <positive-pid>

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy scope put --cgroup-id <positive-cgroup-id>

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy scope get <scopeId-from-put-response> --revision 1

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy scope delete <scopeId-from-put-response> --revision 1

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy binding put \
    --policy-id <policyId> --policy-revision 1 \
    --scope-id <scopeId> --scope-revision 1

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy binding get <bindingId-from-put-response>

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy binding delete <bindingId-from-put-response>

# Every list command accepts --limit (1..=1000, default 100) and --offset.
target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy template list --limit 100 --offset 0

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy scope list

target/debug/asc-cli \
  --socket /run/agent-sec-core/v2/daemon.sock \
  --token-file /run/agent-sec-core/v2/policy-admin.token \
  policy binding list
```

`--limit` is a maximum record count, not a promise that every page has that
many records. List queries also stop before their stored JSON would exhaust the
bounded response-item byte budget; callers continue with
`offset += items.length`. If one record alone cannot fit the response budget,
the method returns `payload_too_large`. SQLite checks each row's byte length
before deserializing it, and transport serialization uses a bounded writer, so
the 4 MiB response contract is not implemented as an after-the-fact check on an
unbounded allocation.

Scope identity follows the Policy identity contract. Omitting `--scope-id`
creates a new daemon-generated UUID at revision 1. Supplying `--scope-id`
updates only that existing Scope; an unknown UUID returns `not_found`. `--pid`
and `--cgroup-id` are mutually exclusive simple selectors. They persist the
caller's unresolved selection intent: this slice does not claim that the PID is
still the same process, that the cgroup exists, or that either value is a
trusted kernel identity. Repeating an identical update is a no-op; changing the
selector receives the next daemon-owned immutable revision.

Binding identity also follows the same create/update split. Omitting
`--binding-id` creates a new daemon-generated UUID; supplying it updates only an
existing Binding. The caller chooses the exact immutable Policy and Scope
revisions, but the daemon allocates the Binding revision and internal reconcile
identity and compare-and-swap precondition. Internal reconcile identities are
not part of the daemon or CLI user contract. Binding PUT and DELETE return the
accepted Binding record, while `policy.bindings.get` is the user-facing query
surface. The CLI only constructs and forwards the DTO. Binding delete requires
exactly one Binding UUID and accepts a new daemon-owned `ABSENT` desired
revision.

The current Binding record exposes desired state only. This slice cannot claim
that a Binding is effective at a PEP because no Adapter receipt or PEP status
protocol exists yet. That future integration must project observed revision and
effective status onto the Binding returned by `policy.bindings.get`; it must not
expose the daemon's internal reconcile work item as a public Operation resource.

ID-less Policy, Scope, and Binding PUTs are creates, not idempotent updates. A
caller that needs retry convergence must retain the returned UUID and use the
corresponding `--policy-id`, `--scope-id`, or `--binding-id` update form.

Commands print only method data as pretty JSON. The socket and token paths are
explicit until packaging freezes the system-owned product paths. A missing
daemon returns `daemon_unavailable`; the CLI does not lower a Policy Template,
open SQLite, start a daemon, or execute a local fallback.

Policy delete always requires both the exact UUID `policyId` and a positive
`revision`; there is no implicit latest revision, all-revisions delete, or
name-based delete. Delete physically removes only that revision's authored
Template and Canonical IR; every other revision of the same Policy remains
queryable even when it has no Binding. A deleted revision returns `not_found`
to GET and to a new Binding request that has not yet resolved that revision.

Scope delete has the same exact-revision behavior and per-Scope allocation
high-watermark as Policy delete. Its revision number is never reused.

Binding uses copy-on-bind rather than live references. The daemon resolves the
request's exact Policy and Scope revisions into a complete immutable
`PreparedBinding` snapshot before entering Runtime persistence. The Runtime SQL
transaction atomically writes only the Binding snapshot, internal reconcile
work item, outbox item, and audit record; it does not query PAP tables. Once resolution
succeeds, concurrent or later deletion of either source revision does not
invalidate the Binding and does not stop enforcement. The retained Binding
continues to expose and dispatch its embedded Policy, Canonical IR, Scope, and
selector. Stopping that desired state requires deleting the Binding itself.

Policy revision numbers are daemon-assigned, monotonically increasing, and
never reused. The store maintains a per-Policy allocation high-watermark apart
from the retained revision content. Deleting revision 2 therefore leaves
revisions 1 and 3 intact, and the next changed PUT creates revision 4. A missing
number is an explicitly deleted revision, not permission to reuse its identity.

CI-backed examples for all currently supported Policy Template kinds live in
[`fixtures/README.md`](fixtures/README.md). The CLI E2E test sends every complete
`template-*.json` sample through a real daemon `PUT -> GET` sequence.

Every `policy.*` NDJSON request requires
`auth={"scheme":"bearer","token":"..."}`. `daemon.health` is the sole
unauthenticated method in this slice. Pre-migration daemon methods intentionally return
`unknown_method`; there is no Python or local fallback.

Requests may carry W3C `traceparent` and `tracestate`. The daemon attaches a
valid remote parent to an OpenTelemetry request span; OpenTelemetry is the sole
authority for TraceId and SpanId. The daemon-generated `request_id` remains a
separate request-correlation field. JSON logs include the OTel-generated trace
and span identifiers as span fields. This slice creates a local SDK provider but
does not choose an OTLP exporter; exporter composition belongs to deployment.

The bearer token is a minimal local management credential, not trusted workload
identity or role-based authorization. Those identity and authorization models
remain outside this work package.

## Verification

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The process-level Policy CRUD suite under [`tests/e2e`](tests/e2e/README.md)
starts the real `asc-daemon` and drives it exclusively through the real
`asc-cli` binary with a file-backed SQLite database. Run it directly with:

```bash
cargo test -p asc-e2e-tests --test policy_crud
```

The canonical method inventory and request examples live in
`fixtures/daemon/policy-methods.json`.
