# AgentSecCore V2 Policy and daemon foundations

This workspace slice contains the dependency-light contracts, Policy
Administration Point, first-version PAP daemon protocol, product Policy-template
compiler, protocol-independent Unix-domain-socket service framework, and runnable
foreground process bootstrap, together with the first AgentSight file-deletion
target Adapter, and its independent deployment Client used by later AgentSecCore
V2 work packages. It deliberately contains no durable persistence, Policy runtime,
reconciliation scheduling worker, daemon reconciliation wiring, or outbox.
The synchronous single-attempt reconciliation core is available as `asc-pcp`.

The Rust `agent-sec-cli` exposes all 15 Policy, Scope and Binding CRUD commands through
an explicit daemon socket. Its Cargo package and source directory remain `asc-cli`;
the executable target is `agent-sec-cli`. See the [CLI reference](../../../docs/user-guide/en/agent-security/agent-sec-core/policy-cli.md)
and [CLI acceptance record](../docs/design/POLICY_CLI_ACCEPTANCE_zh.md).

The current crates are:

- `asc-foundation-types`: bounded transport-independent identifiers and revisions.
- `asc-policy-types`: authored Policy and immutable prepared Policy/Scope/Binding
  snapshots, backend-independent IR, and target Adapter contracts.
- `asc-policy-engine`: deterministic `prevent_file_deletion` authoring-template
  compiler with a frozen Canonical Policy IR golden. Other template kinds remain
  explicitly unsupported until their lowering and Adapter evidence are defined.
  The implemented template covers path-entry deletion only; rename, move, and
  other namespace mutations are outside its contract.
- `asc-policy-adapter-agentsight`: deterministic file-deletion and PID-Scope
  translation into an AgentSight/ActPlane plan, with semantic and encoding checks.
  Compiler acceptance belongs to the deployed target, not an embedded compiler.
- `asc-agentsight-client`: health-gated AgentSight apply/delete transport for
  one configured endpoint, with process identity resolution and complete HTTP
  fixtures. It does not depend on a reconciliation framework.
- `asc-policy-target-contracts`: shared, PEP-neutral `TargetBindingAdapter` and
  `TargetDeploymentClient` ports; data lives in `asc-policy-types::target`.
- `asc-policy-repository`: shared Binding aggregate data, consistent reads and
  atomic snapshot CAS; independent of reconciliation implementation.
- `asc-pcp`: synchronous single-attempt `BindingReconciler::reconcile` core over
  repository, Adapter and Client ports. Event delivery, timers and daemon wiring
  remain separate work packages.
- `asc-pap`: transport-independent current-record Policy/Scope/Binding CRUD with
  monotonic revisions over explicit compiler and repository ports.
- `asc-pap-repository-memory`: explicitly temporary process-local Repository
  adapter used only to keep daemon/PAP integration runnable before durable
  persistence lands; also implements aggregate reads/CAS over PAP's Binding map.
- `asc-daemon-protocol`: strict request/response contracts and an explicit
  allowlist for 15 Policy, Scope, and Binding administration methods.
- `asc-daemon-handler`: inbound protocol adapter that decodes daemon requests,
  applies server-owned authorization, routes PAP methods, and projects protocol
  responses without depending on a concrete Repository or compiler.
- `asc-daemon-core`: trusted Principal construction boundary and the
  `PolicyAdministration` application port. `PapService<R, C>` implements this
  port directly, so repository/compiler generics do not leak into dispatch.
- `asc-daemon-service`: bounded UDS admission, one-request framing, kernel peer
  credentials, dispatcher/rejection-encoder injection, connection isolation,
  dispatch cancellation, and controlled drain.
- `asc-daemon-client`: synchronous UDS client preserving complete responses, with
  a single connect/write/read deadline and no retries or local fallback. It uses
  standard-library blocking I/O and `socket2` for bounded connect; neither it nor
  the CLI binary requires a Tokio runtime.
- `asc-cli`: command parsing, typed Policy request construction and Policy output;
  server dependencies are test-only. `commands.rs` registers and dispatches the
  top-level commands; `commands/{policy,scope,binding}.rs` own their arguments and
  request mappings, with pagination and encoding helpers in `commands/common.rs`.
- `asc-daemon`: foreground process and composition root that configures and
  injects concrete adapters into the daemon service.

The crate relationships, acceptance types, executable pass/fail matrix,
compatibility report, direct-consumer evidence, and rollback boundary are recorded
in [`PAP_DAEMON_API_ACCEPTANCE_zh.md`](../docs/design/PAP_DAEMON_API_ACCEPTANCE_zh.md).

The [scan capability development guide (Chinese)](../docs/design/V2_SCAN_CAPABILITY_DEVELOPMENT_GUIDE_zh.md)
maps Prompt Scan and Code Scan migration work onto this checkout, including module
locations, dependency order, interface boundaries, and acceptance requirements.
It describes planned work; this workspace does not yet expose scan methods.

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
`asc-daemon-handler::DaemonDispatcher` receives a bounded raw request frame and
owns method routing. Method
allowlist routing is internal to that one dispatcher implementation; it is not a
second service dispatch layer. A separate `RejectionEncoder` receives typed
transport failures and must remain independent of PAP/Repository state.

The PAP request path is:

```text
UDS frame -> DaemonDispatcher -> method metadata authorization
          -> PapHandler -> PolicyAdministration -> PapService<R, C>
```

RPC reuses the domain `PolicyTemplate`, `ScopeSelector`, `PreparedPolicy`,
`PreparedScope`, and `BindingView` types. It does not define result wrappers for
each CRUD operation. `PolicyAdministration` intentionally mirrors the use cases
once: it erases `R`/`C` before dispatch and repeats authorization at the
application boundary; there is no additional `PapServiceAdapter` forwarding
object.

The current bootstrap bounds frame read, application dispatch, rejection
encoding, response write, connection drain, and final Tokio runtime shutdown.
Dispatch timeout releases transport capacity and signals cooperative cancellation;
it cannot forcibly stop an application blocking call that ignores that signal.
The framework also cannot prove that a concrete PAP/Repository avoids global
locks; that remains a required direct-consumer concurrency test at integration.

The current `asc-daemon` executable composes and registers the PAP dispatcher and
protocol rejection encoder from `asc-daemon-handler`. It composes `PapService`
with `PolicyTemplateCompiler`, a
root-managed Principal policy, and an explicitly transitional process-local
Repository. Policy CRUD therefore works during one daemon lifetime, but all
state disappears on restart; this is integration evidence, not durable
persistence or distribution readiness. The process prints that limitation at
startup. It also requires an explicit absolute socket path because
packaging-owned system paths, singleton/stale-socket policy, runtime directory
hardening, and readiness remain later process-integration work.

UID 0 is always a Policy administrator. A deployment operator can add other UIDs
at startup with repeatable `--policy-admin-uid <UID>` options. Omitted means root
only. Configured administrators cannot delegate other UIDs at runtime; that API
still requires root. The allowlist is process-local and must be supplied on each
startup. Configuration-file loading, persistence and management RPCs remain later
work. Authorization does not change OS socket permissions or deployment topology.

Run the independent transport process in the foreground:

```bash
cargo run -p asc-daemon -- serve --socket /absolute/existing-directory/daemon.sock
```

`asc-daemon-handler::DaemonDispatcher` implements `RequestDispatcher` directly
and is injected by the executable composition root together with
`JsonRejectionEncoder`. PAP is one
registered method family inside the dispatcher; the service framework and
rejection path remain independent of PAP, its compiler, and its repository.

## PAP RPC contract

The closed method inventory contains create, update, exact get, bounded list,
and delete for each of Policy, Scope, and Binding. Successful responses are
`{requestId,result}` and failures are `{requestId,error}`. The result is the
domain record itself; list is the sole shared `{items,total}` shape. Exact inputs
and output type names are frozen by
`asc-daemon-protocol/tests/fixtures/pap-methods.json`.

The stateful `asc-daemon-protocol/tests/fixtures/pap-crud-e2e.json` scenario
freezes complete request and response values for all 15 methods, including
Canonical Policy IR, Scope templates, embedded Binding snapshots, revisions,
statuses, and deterministic digests. Server-generated request and resource UUIDs
use named placeholders so the same fixture can assert their format and identity
flow across later requests. A UDS integration E2E always executes the complete
scenario with a server-authorized test principal. The `asc-daemon` bootstrap E2E
also starts the real binary with `--policy-admin-uid` set to the test UID and
executes the complete scenario without root. A separate default-config case
verifies non-root `permission_denied` (or full CRUD when root). CLI process tests
use an in-process daemon service; a combined CLI and daemon binary E2E is deferred.

Binding create/update accepts desired state and returns `PENDING_APPLY`; delete
returns `PENDING_DELETE`. These responses prove PAP acceptance only. They do not
mean target enforcement or deletion completed. LIST is integration-ready but is
not distribution-ready until a server-owned aggregate encoded-byte budget is
passed through Repository, PAP, and transport.

TODO(policy-response-bounds): direct `PreparedPolicy` and `BindingView` mutation
results can exceed the response-frame limit after process-local state has already
changed, while embedded snapshots can also make Binding GET/LIST oversized.
Before a durable Repository or distribution gate, converge the public result and
storage shapes and enforce server-owned encoded-size budgets for mutations,
single-record GET, and LIST; increasing the transport limit alone is not the fix.

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

`PreparedBinding` is an immutable Policy/Scope snapshot. `(binding_id,
binding_revision)` identifies that spec; `BindingView { spec, status }` projects
its current lifecycle. Only spec changes increment `bindingRevision`.

| Current | Request | Result | Revision |
|---|---|---|---|
| absent | CREATE | fresh server-generated ID, `PENDING_APPLY` | 1 |
| `PENDING_APPLY`, `APPLYING`, `READY` | identical UPDATE | no-op | unchanged |
| `APPLY_FAILED` | identical UPDATE | `PENDING_APPLY`, reset retry controls, retain prepared request | unchanged |
| `PENDING_APPLY`, `READY`, `APPLY_FAILED` | changed-spec UPDATE | `PENDING_APPLY`, clear prepared request, retain cleanup targets | +1 |
| `APPLYING` | changed-spec UPDATE | `OperationInProgress` | unchanged |
| `PENDING_DELETE`, `DELETING`, `DELETE_FAILED` | any UPDATE | `OperationInProgress`; deletion is irreversible | unchanged |
| Apply-side states, `DELETE_FAILED` | DELETE | `PENDING_DELETE`, reset retry controls, retain spec/prepared/targets | unchanged |
| `PENDING_DELETE`, `DELETING` | DELETE | no-op | unchanged |
| absent | GET / UPDATE / DELETE | `NotFound` | — |

Workers claim pending work as `APPLYING` or `DELETING`. Apply success becomes
`READY`; retryable failure returns to the corresponding pending state with a
deadline; permanent/exhausted failure becomes `APPLY_FAILED` or `DELETE_FAILED`.
Delete success atomically removes the Binding and all runtime data only after
all targets are confirmed absent. `Deleted` remains an internal completion marker
in the state machine, never a persisted current status. LIST omits removed rows.
Re-deployment uses CREATE with a new ID at revision 1.

PAP writes compare the complete expected Binding under the same transaction as
request admission. `update_binding(None, next)` inserts a fresh ID;
`update_binding(Some(expected), next)` updates only an existing record. It cannot
resurrect a record removed between the service read and repository write.
Reconciler aggregate CAS also compares runtime and deployments; it cannot erase
a newer intent or target observation. A Delete accepted while Apply is running
keeps the same revision, and the old Apply still records its target observations
before the next cleanup attempt.

PAP request semantics and the synchronous reconciler are tested together using
the memory repository. The daemon still has no notification/timer worker wired
to accepted requests; PAP acceptance does not imply target completion. Durable
storage and cross-process recovery remain separate work packages.

Durable persistence, Policy runtime, reconciliation scheduling worker, and
outbox belong to later work packages and are intentionally absent
from this slice. The compiler included here is limited to the one golden-backed
`prevent_file_deletion` lowering described above.

Dependency sources, TLS/unsafe boundaries and release audit requirements are
recorded in [DEPENDENCIES.md](DEPENDENCIES.md).

Run the branch-owned validation from this directory:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Native OpenTelemetry tracing

This is the first OTel integration. V1/V2 refer to the Python/Rust product
implementations, not OTel generations. Existing `--trace-context` JSON and record
metadata remain supported inputs. `bind_trace_context_input` maps the former into
the unified OTel Context. Caller-supplied opaque trace/invocation labels retain
their correlation meaning separately from SDK TraceId/SpanId.

`asc-observability` supplies the current OTel Context, five Agent baggage fields,
read-only correlation snapshots and a process-owned `runtime` feature. CLI/client,
daemon, PAP and compiler spans share SDK identity. A raw UDS caller that omits
context gets a fresh daemon root. Missing Agent metadata is allowed for ordinary
PAP calls; future observability consumers call `validate_metadata` when required.
Adapters for the existing V1 record input use `bind_metadata(parent, value, kind)` with `AgentRun`,
`ModelCall` or `ToolCall`. It validates the record's own V1 metadata before replacing
session/run/call/tool fields: missing required fields fail even if the parent has
them; omitted/null optional fields clear inherited values. Trace parentage,
request correlation, compatibility labels and independent agent attribution remain.
Metadata extras are ignored per hook schema; `agent_name` comes from trace-context
or the native carrier. Ordinary child-context propagation continues to inherit.
The production runtime uses a real SDK with fixed `AlwaysOff` sampling for local
correlation. IDs, parentage and baggage remain available; no exporter is installed.

Existing caller input remains flat JSON; put this bootstrap option before command
names and before other options' non-option values, matching the V1 parser:

```bash
agent-sec-cli --trace-context '{"agent_name":"openclaw","session_id":"session-123","tool_call_id":"tool-1"}' \
  --socket /run/agent-sec-core/daemon.sock policy list
```

A native upstream parent may be supplied alongside it:

```bash
agent-sec-cli --trace-context '{"session_id":"session-123"}' \
  --otel-context '{"version":1,"traceparent":"00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"}' \
  --socket /run/agent-sec-core/daemon.sock policy list
```

The current release exposes no OTLP exporter or exporter configuration. `OTEL_*`
export, sampling and batch settings cannot enable export or change the fixed
local sampling policy. No Collector, HTTP client or exporter worker is created.

| Variable | Behavior |
| --- | --- |
| `RUST_LOG` | Default warn; `info` enables bounded JSON correlation diagnostics on stderr; `off` suppresses these records without disabling context |
| `AGENT_SEC_INVOCATION_ID` | Optional caller-supplied invocation label; never automatically generated |

Service resources use `asc-daemon` / `agent-sec-cli` and the build package version.
Each runtime owns one diagnostic worker with a 64-record queue and a 32 KiB
per-record limit (2 MiB queued payload). It also handles daemon startup warnings
and operational errors, independently of `RUST_LOG`. Producers never wait for
stderr I/O; overflow, oversized records, worker creation failure and sink failures
lose diagnostics. There is no per-second rate limit; the stderr consumer owns
retention and rotation. CLI draining waits at most 50 ms; daemon draining shares
the additional 2 s provider shutdown budget after service/runtime shutdown.
`init_runtime` is called once from main. Subscriber conflicts or invalid SDK
identity fail before business work with exit 1; `otel: <reason>` is best effort,
using a bounded worker even before successful runtime initialization.
The process panic hook also queues only `runtime: panic`, without payloads;
unwind/abort behavior is unchanged.
CLI help, usage, errors and business results retain synchronous output semantics.
These required outputs can wait for their consumer; the diagnostic queue is not a
lossy replacement for business output.

Native requests support optional `traceContext` (version 1, optional string
`traceparent`, `tracestate`, `baggage`) and `compatibility` (version 1, optional
`traceId`, `invocationLabel`). The new CLI requires a daemon supporting this
carrier; pre-carrier daemons are outside the supported version matrix. Deploy
server first; a client never retries without context after rejection. Regular
RPCs inject a carrier even without tracing flags. The preserved `--trace-context`
input and explicit `AGENT_SEC_INVOCATION_ID` can affect wire attribution/labels. Requests have separate 4 MiB business and 32 KiB
propagation budgets; response capacity remains 4 MiB, including LF.

Business functions use `tracing::info_span!` or `#[tracing::instrument(skip_all)]`;
names are defined at each callsite. For a task/thread boundary, capture
`asc_observability::Context::current()`, bind a fresh child using `parent_span`,
and instrument the future. Do not hold an entered span/Context guard across await.
Consumers call `snapshot()` inside the scope, then persist that read-only snapshot
independently of span sampling/export. This package does not implement event
storage or local trajectory reconstruction.

Validation and rollback: [OTel acceptance](../docs/design/V2_OTEL_ACCEPTANCE_zh.md).

The AgentSec UDS adapter accepts up to 16 KiB of encoded baggage, preserving all
five 256-code-point values. Non-ASCII bytes must be percent-encoded; using fewer
ASCII escapes does not reduce their size. Receivers supporting only the W3C
8192-byte interoperability minimum may drop larger headers; the locked SDK's
standard BaggagePropagator drops them whole. Cross-service forwarding must define
its own budget/compatibility contract before use. The local adapter does not
silently shrink original metadata to meet another receiver's limit.

After building the V2 workspace, run the cases from the component directory:

```bash
uv run --project agent-sec-cli pytest tests/v2/e2e/test_otel_e2e.py -v
```

These tests require Linux, UDS, loopback TCP and subprocess support. Missing
binaries or unavailable sockets fail; only the root-inapplicable non-root
authorization case explicitly skips. Shared Makefile/CI integration and V1 test
collection boundaries are handled by the separate V2 E2E integration PR.
