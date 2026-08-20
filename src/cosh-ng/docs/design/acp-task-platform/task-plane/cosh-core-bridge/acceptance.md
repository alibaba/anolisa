# Phase 1 Cosh Core Bridge Acceptance Baseline

[中文版](acceptance_zh.md) | [Design](design.md)

## Baseline result

**Overall: PARTIAL implementation on a candidate based on upstream
`a6592234`; Phase 1 remains NOT ACCEPTED.** The candidate adds a neutral
`AgentRuntimePort` and a supervised `CoshCoreBridge` over two explicitly private
COSH profiles: legacy Shell/Core v1 and Gateway brokered v3. The bridge fences
public identity and event order, bounds retained state, and settles cancellation
through process cleanup. The contained Gateway production profile is task-only
and exposes only `ask_user_question`. It has no production `ExecutionTarget`
and no checkpoint/ws-ckpt dependency. Generic Capability/Permit/Execution
contracts remain future foundations. Broader tool execution, resume/recovery,
Shell ownership migration, real-provider evidence, and manual Terminal
validation remain unaccepted.

## Result vocabulary

| Result | Meaning |
| --- | --- |
| PASS | Baseline evidence satisfies a reusable or final criterion exactly. |
| PARTIAL | A scoped foundation is implemented and tested, but integration or required failure evidence is absent. |
| FAIL | Current behavior contradicts the target production invariant. |
| NOT IMPLEMENTED | The required Gateway path does not exist. |
| BLOCKED | A named prerequisite decision prevents validation. |

## Evidence inspected

- Upstream source baseline: `a6592234`.
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
  implements strict private v1/v3 initialization and typed wire observations without ACP naming.
- [`profile.rs`](../../../../../crates/cosh-gateway-contracts/src/profile.rs) pins the
  `task-only-v1` manifest identity, governed target, and exact Runtime inventory.
- [`runtime/port.rs`](../../../../../crates/cosh-gateway/src/runtime/port.rs) defines the
  provider-neutral, object-safe command/event boundary and redacted errors.
- [`runtime/cosh_core_bridge.rs`](../../../../../crates/cosh-gateway/src/runtime/cosh_core_bridge.rs)
  binds COSH identities, maps bounded public events, rejects unsupported control requests, and
  owns one supervisor generation without importing Task storage, core, or Shell crates.

## Acceptance matrix

| ID | Criterion | Baseline | Evidence or missing artifact |
| --- | --- | --- | --- |
| CCB-001 | Bridge implements neutral `AgentRuntimePort`. | PASS for library slice | Object-safe port and Core implementation compile and pass focused lifecycle tests. |
| CCB-002 | Private COSH v1/v3 are explicitly distinct from ACP v1. | PASS | The shared dual-version corpus and both codecs use COSH names and versions; neither profile is presented as ACP. |
| CCB-003 | Exact initialization succeeds before Task input admission. | PARTIAL | Gateway negotiates brokered v3 before Prompt and requires `SessionOpened` first; the daemon persists Runtime binding before prompt, while complete Core recovery remains. |
| CCB-004 | Gateway production rejects legacy, missing, and mismatched negotiation. | PASS | Cross-implementation negative fixtures reject wrong/missing version, execution profile, capability-profile identity, Runtime inventory, and capability before input; production requires exact brokered v3. |
| CCB-005 | `RuntimeSupervisor` solely owns child process lifecycle. | PARTIAL | New supervisor owns one child/group/pipes/reap; existing Shell core owner and restart policy are not migrated. |
| CCB-006 | Every JSONL message maps to a bounded ordered Runtime event/command. | PARTIAL | Session, text, tool observation, result, cancel, and transport failure map with monotonic sequence; question/auth/tool permission, usage, environment, durable backpressure, and full goldens remain. |
| CCB-007 | Task/Run/runtime/Agent/provider IDs remain distinct. | PARTIAL | The daemon persists fenced Runtime binding and rejects stale generation; complete provider-session recovery remains. |
| CCB-008 | Bridge never writes Task storage. | PASS for library slice | Dependency and source review show no storage owner or storage calls in the port/bridge. |
| CCB-009 | The enabled Gateway brokered profile prevents core-local side effects. | PASS for task-only scope | Its immutable inventory contains only side-effect-free `ask_user_question`; extension, Skill, MCP, hook, Shell, file, process, network, and checkpoint paths are absent or disabled. This is not a universal Broker claim. |
| CCB-010 | Every side effect enabled in that profile reaches Broker and a permit-bound typed result. | NOT IMPLEMENTED | No production side-effecting operation or `ExecutionTarget` is enabled in the task-only profile. |
| CCB-011 | Approval receipt follows durable Task ownership. | PARTIAL | Task-owned question/input state is durable; approval-to-permit and execution-result dispatch remain future capability work. |
| CCB-012 | Question/auth/evidence use durable or secret-safe ports. | PARTIAL | Runtime v4 input requests enter durable exact-pending Task state; the private typed dispatch row holds the raw response while Task events and receipts hold only its digest. Core auth/evidence and broader question mappings still fail closed or remain absent. |
| CCB-013 | Process cancel escalates, kills the group, and reaps children. | PARTIAL | Focused tests cover interrupt, cancelled terminal, TERM/KILL/reap, and synchronous fallback cleanup; descendant and cancel/result/EOF race fixtures remain. |
| CCB-014 | Provider session persists separately from Task storage. | PASS | Current `SessionStore` is workspace-scoped provider state. |
| CCB-015 | Crash/restart never silently resends an uncertain prompt. | PARTIAL | Runtime binding/restart convergence is durable and fail closed; side-effect uncertainty reconciliation and complete prompt/recovery fixtures remain future work. |
| CCB-016 | Gateway and Core preserve the one-way process/wire boundary. | PASS | `cosh-gateway` has no core/Shell crate dependency, and `cosh-core` has no Gateway crate dependency. Each owns its private wire shape, with the shared golden corpus detecting drift. |
| CCB-017 | Phase 1 brokered inventory and private-protocol profile decision are frozen. | PASS for scope decision | Gateway production uses private COSH v3 with the pinned `task-only-v1` identity and exposes only `ask_user_question`. Legacy v1 stays with standalone Shell; checkpoint/ws-ckpt and Shell attachment/owner migration are future work. |

Legacy Shell behavior and ACP `doctor`/`run` interoperability are ungoverned compatibility paths,
not proof of a Gateway-governed path.

## Required fixtures, commands, and artifacts

| Artifact | Required proof |
| --- | --- |
| `cosh-private-wire-dual-version` canonical corpus | Legacy v1 initialize/ack, brokered v3 task/question request/ack/result, and wrong/missing version/profile/manifest/inventory/capability cases. |
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
cargo test --package cosh-gateway cosh_core_jsonl
cargo test --package cosh-core --test jsonl_protocol
cargo test --package cosh-gateway-contracts runtime_schema
```

Current scoped evidence on the rebased candidate:

```bash
cargo test --locked -p cosh-gateway-contracts profile
cargo test --locked -p cosh-core brokered_profile
cargo test --locked -p cosh-core private_wire_dual_version_corpus_matches_core_types
cargo test --locked -p cosh-gateway runtime_tool_inventory
cargo test --locked -p cosh-gateway stale_validated_outbox_attempt_is_normal_contention
cargo test --locked -p cosh-gateway invalid_start_intents_are_rejected_before_outbox_claim
cargo test --locked -p cosh-gateway exact_task_only_v2_intent_maps_to_current_profile
cargo clippy --locked -p cosh-gateway-contracts --all-targets -- -D warnings
cargo clippy --locked -p cosh-core -p cosh-gateway --all-targets -- -D warnings
cargo doc --locked --no-deps -p cosh-gateway-contracts
bash scripts/check-source-layout.sh
```

This covers manifest and inventory admission, Core/Gateway dependency isolation, exact legacy-v2
mapping, stale Outbox-attempt contention, and the shared dual-version wire corpus. It does not
replace full package/workspace, process-tree/race, universal Broker, recovery, backpressure,
real-provider, or PTY gates; broader coverage remains delegated to CI.

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
