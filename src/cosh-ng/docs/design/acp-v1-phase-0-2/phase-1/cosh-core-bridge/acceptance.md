# Phase 1 Cosh Core Bridge Acceptance Baseline

[中文版](acceptance_zh.md) | [Design](design.md)

## Baseline result

**Overall: PARTIAL implementation on a candidate based on upstream
`e3763b001c91f3c13dc6afbd57aac924162e9f59`; Phase 1 remains NOT ACCEPTED.** The candidate adds a
neutral `AgentRuntimePort` and a supervised `CoshCoreBridge` over the strict private cosh-core
JSONL v1 codec. The bridge now fences public identity and event order, bounds retained state, and
settles cancellation through process cleanup. Durable coordination, brokered tool execution,
resume/recovery, Shell ownership migration, and real-provider evidence remain absent.

## Result vocabulary

| Result | Meaning |
| --- | --- |
| PASS | Baseline evidence satisfies a reusable or final criterion exactly. |
| PARTIAL | A scoped foundation is implemented and tested, but integration or required failure evidence is absent. |
| FAIL | Current behavior contradicts the target production invariant. |
| NOT IMPLEMENTED | The required Gateway path does not exist. |
| BLOCKED | A named prerequisite decision prevents validation. |

## Evidence inspected

- Upstream source baseline: `e3763b001c91f3c13dc6afbd57aac924162e9f59`.
- [`protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs) defines exact private protocol v1
  and all current message shapes.
- [`headless.rs`](../../../../../crates/cosh-core/src/headless.rs) negotiates and runs provider turns.
- [`session.rs`](../../../../../crates/cosh-core/src/session.rs) and
  [`session/store.rs`](../../../../../crates/cosh-core/src/session/store.rs) persist provider
  conversations.
- [`cosh_core_service.rs`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service.rs) owns
  the current Shell persistent process and cancellation lifecycle.
- [`control_protocol.rs`](../../../../../crates/cosh-shell/src/adapter/control_protocol.rs) mirrors
  parser/serializer behavior inside standalone Shell.
- [`runtime/supervisor.rs`](../../../../../crates/cosh-gateway/src/runtime/supervisor.rs) owns one
  child process group, bounded pipes, TERM/KILL escalation, reap, and process terminal delivery.
- [`runtime/bounded_io.rs`](../../../../../crates/cosh-gateway/src/runtime/bounded_io.rs) implements
  bounded stdout framing and stderr-tail retention.
- [`runtime/cosh_core_jsonl.rs`](../../../../../crates/cosh-gateway/src/runtime/cosh_core_jsonl.rs)
  implements strict private v1 initialization and typed wire observations without ACP naming.
- [`runtime/port.rs`](../../../../../crates/cosh-gateway/src/runtime/port.rs) defines the
  provider-neutral, object-safe command/event boundary and redacted errors.
- [`runtime/cosh_core_bridge.rs`](../../../../../crates/cosh-gateway/src/runtime/cosh_core_bridge.rs)
  binds COSH identities, maps bounded public events, rejects unsupported control requests, and
  owns one supervisor generation without importing Task storage, core, or Shell crates.

## Acceptance matrix

| ID | Criterion | Baseline | Evidence or missing artifact |
| --- | --- | --- | --- |
| CCB-001 | Bridge implements neutral `AgentRuntimePort`. | PASS for library slice | Object-safe port and Core implementation compile and pass focused lifecycle tests. |
| CCB-002 | Private JSONL v1 is explicitly distinct from ACP v1. | PASS | Current runtime contract states the separation. |
| CCB-003 | Exact initialization succeeds before Task input admission. | PARTIAL | Bridge negotiates before Prompt and requires `SessionOpened` delivery first; durable Task admission is not integrated. |
| CCB-004 | Gateway production rejects legacy unversioned peers. | PARTIAL | Bridge uses the strict codec, which rejects missing/mismatched versions; installed Core profile evidence remains. |
| CCB-005 | `RuntimeSupervisor` solely owns child process lifecycle. | PARTIAL | New supervisor owns one child/group/pipes/reap; existing Shell core owner and restart policy are not migrated. |
| CCB-006 | Every JSONL message maps to a bounded ordered Runtime event/command. | PARTIAL | Session, text, tool observation, result, cancel, and transport failure map with monotonic sequence; question/auth/tool permission, usage, environment, durable backpressure, and full goldens remain. |
| CCB-007 | Task/Run/runtime/Agent/provider IDs remain distinct. | PARTIAL | Bridge creates a fenced in-memory binding with scoped provider `ExternalRef`; durable binding persistence and stale-generation reconciliation remain. |
| CCB-008 | Bridge never writes Task storage. | PASS for library slice | Dependency and source review show no storage owner or storage calls in the port/bridge. |
| CCB-009 | Brokered profile prevents core-local side effects. | FAIL | Current allowed/approved tools can execute in core. |
| CCB-010 | `can_use_tool` reaches Broker and a permit-bound target result. | NOT IMPLEMENTED | Broker/Bridge absent. |
| CCB-011 | Approval receipt follows durable Task ownership. | NOT IMPLEMENTED | Current receipt proves Shell main-thread receipt only. |
| CCB-012 | Question/auth/evidence use durable or secret-safe ports. | PARTIAL | Core auth and all unmodeled control requests fail closed with redacted errors; durable ports remain absent. |
| CCB-013 | Process cancel escalates, kills the group, and reaps children. | PARTIAL | Focused tests cover interrupt, cancelled terminal, TERM/KILL/reap, and synchronous fallback cleanup; descendant and cancel/result/EOF race fixtures remain. |
| CCB-014 | Provider session persists separately from Task storage. | PASS | Current `SessionStore` is workspace-scoped provider state. |
| CCB-015 | Crash/restart never silently resends an uncertain prompt. | NOT IMPLEMENTED | Task/Broker reconciliation absent. |
| CCB-016 | Gateway has no Rust dependency on core implementation or Shell. | PASS | `cosh-gateway` speaks mirrored private wire types and has no core/Shell crate dependency. |
| CCB-017 | Brokered tool inventory and private-protocol extension decision are frozen. | BLOCKED | Core/Broker owner decision pending. |

PASS entries for current Shell behavior are reusable baseline evidence, not proof that the future
Gateway-owned path exists.

## Required fixtures, commands, and artifacts

| Artifact | Required proof |
| --- | --- |
| `cosh-jsonl-v1` canonical corpus | Every input/output, optional capability, malformed and oversized case. |
| Cross-implementation fixture report | Core encoder, Shell mirror, and Gateway decoder agree. |
| `runtime-supervisor-killpoints` | Spawn, negotiate, stream, cancel, EOF, wait, shutdown, restart races. |
| `runtime-event-mapping` goldens | Bounded normalized events and ID correlation for every message. |
| `brokered-tool-inventory` | Every exposed side-effecting tool delegates or is disabled. |
| Provider-session recovery matrix | New, resume, mismatch, corrupt, stale, cancel, restart. |
| Backpressure fixture | Durable sink outage never drops control or terminal events. |

Expected scoped commands after implementation are:

```bash
cargo test --package cosh-gateway cosh_core_bridge
cargo test --package cosh-gateway runtime_supervisor
cargo test --package cosh-gateway cosh_jsonl_contract
cargo test --package cosh-gateway-contracts runtime_schema
```

Current bridge-targeted evidence on the uncommitted candidate:

```bash
cargo +1.88.0 test --locked --package cosh-gateway cosh_core_bridge
# 7 passed; 0 failed; 104 filtered out in the library target
```

This covers identity fencing, stream and terminal mapping, single terminal delivery, open timeout,
cross-Run rejection, SessionOpened-before-Prompt ordering, idle-cancel rejection, aggregate Prompt
bounds, retained tool-ID bounds, and process cleanup on cancellation. It does not replace the
required canonical corpus, full process-tree/race, Broker, recovery, backpressure, Shell protocol,
real-provider, or PTY gates. Rustdoc and Clippy evidence are recorded only after their final scoped
commands complete.

## Exit criteria

1. CCB-001 through CCB-016 are PASS and CCB-017 has an accepted profile/version decision.
2. Canonical fixture, mapping, process-race, session-recovery, Broker bypass, and backpressure suites
   pass at the exact candidate commit with recorded counts.
3. A dependency check proves Gateway does not link the core implementation or standalone Shell,
   and the Bridge/RuntimeSupervisor cannot write Task storage or execute OS work outside Broker.
4. Security review covers executable/workspace pinning, environment allowlist, protocol parser
   limits, correlation, secret/auth flow, provider session scope, approval receipt timing,
   cancellation, and uncertain execution.
5. The report records executable/profile configuration, private protocol version, exact commands,
   fixtures, unsupported tools, restart policy, untested real-provider paths, and rollback.

## Current risks

- Reusing Shell `AgentAdapter` types would import presentation and CommandBlock coupling.
- Calling private JSONL “ACP” would create false interoperability and version assumptions.
- Sending generic allow for a side-effect tool bypasses target-bound permits.
- Persisting a provider session binding from a stale Run can attach future work to the wrong Task.
- Reading faster than durable Task event commit can lose control events on daemon crash.
- `ExternalRef.value` contains private provider data and must not be logged or copied to general
  audit output; durable storage still needs an encrypted reference row or keyed digest policy.
