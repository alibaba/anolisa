# Phase 2 ACP Client Bridge Acceptance Report

[中文版](acceptance_zh.md)

Related design: [ACP Client Bridge design](design.md).

## 1. Report scope

- Upstream baseline reviewed: `e3763b001c91f3c13dc6afbd57aac924162e9f59`
- Review date: 2026-08-13
- Change type: first library implementation slice plus design evidence
- Implementation acceptance: **NOT ACCEPTED**

This report records current readiness and the evidence required to exit Phase
2. It does not claim production ACP support or an installed runtime entrypoint.
The narrower [local ACP MVP report](../../phase-1/acp-mvp/acceptance.md) tracks
the first usable local path separately.

## 2. Baseline evidence

The baseline contains a shell-owned `AgentAdapter`, streamed `AgentEvent`
types, a cosh-core adapter, an internal JSONL protocol, and provider session
persistence. It contains no ACP dependency, ACP client, ACP JSON-RPC router,
ACP stdio process, capability negotiation, or conformance suite.

`CONTROL_PROTOCOL_VERSION = 1` in cosh-core is an internal shell-to-core
contract. It is not evidence of ACP `protocolVersion: 1` support.

The candidate worktree adds official `agent-client-protocol = 2.0.0`, raises
the cosh-ng Rust/RPM baseline to 1.88, and implements `AcpV1Codec` plus
`AcpV1RuntimeBridge` and fixed installed-adapter profiles. The bridge embeds
the sole `RuntimeSupervisor` lifecycle implementation
and has focused fixtures for v1 negotiation, a supervised stdio exchange,
session/prompt/update, permission correlation, cancellation settlement,
identity mismatch, unsupported callbacks, and malformed/oversized frames.
It also defines the shared object-safe `AgentRuntimePort` and an ACP adapter
that maps bounded text/tool events, delegates permission normalization to a
trusted port, correlates one-shot decisions, fences public identity, and
settles the supervised child before publishing a terminal event.

## 3. Current readiness

| Area | Baseline status | Acceptance status | Evidence needed to pass |
| --- | --- | --- | --- |
| Neutral `AgentRuntimePort` | Not present in production | **PARTIAL** | Shared object-safe port plus Core and ACP implementations exist; coordinator integration and complete fixtures remain |
| ACP SDK/toolchain ADR and dependency | Not present | **PASS for first slice** | SDK 2.0.0 and Rust 1.88 are pinned; release/license review remains a PR gate |
| Built-in runtime profiles | Not present | **PARTIAL** | Installed entrypoint, pinned Adapter bundle, canonical paths, and environment allowlists have focused tests; signed/offline distribution policy remains |
| ACP v1 initialization | Not present | **PARTIAL** | Exact v1 request, response, and wrong-version rejection pass focused tests; real-Agent conformance remains |
| Capability snapshot | Not present | **PARTIAL** | Stable capability copying and additional-root gating exist; complete method matrix remains |
| stdio transport | Internal JSONL only | **PARTIAL** | Fake Agent exchange uses the sole hardened supervisor; crash/backpressure suite remains |
| ACP session binding | Provider session state only | **PARTIAL** | ACP session is exposed only as a scoped digest under COSH-owned binding IDs; coordinator durability remains |
| Prompt and update mapping | Shell-specific events only | **PARTIAL** | Bounded text, tool observations, and stop reasons map to neutral ordered Runtime events; complete update goldens remain |
| Permission callback governance | Shell approval bridge only | **PARTIAL** | Trusted normalizer produces a Capability request and Broker results select only correlated one-shot choices; production Broker wiring remains |
| Filesystem callbacks | No ACP callback path | **NOT IMPLEMENTED** | Broker-only read/write tests and escape PoCs |
| Terminal callbacks | No ACP callback path | **NOT IMPLEMENTED** | Governed execution handle lifecycle tests |
| Cancellation settlement | Provider-specific cancellation exists | **PARTIAL** | Pending permission callbacks receive ACP cancelled outcomes; prompt/process race suite remains |
| Load/resume/replay | cosh-core provider resume only | **NOT IMPLEMENTED** | Capability-gated ACP load/resume tests |
| Runtime supervision | Shell-owned process lifecycle | **PARTIAL** | ACP reuses `RuntimeSupervisor`; restart, lease-loss, and recovery remain |
| Conformance suite | Not present | **PARTIAL** | Official SDK types and focused fixtures pass; upstream conformance corpus/real Agent remains |

The candidate proves the basic ACP v1 transport shape, but it does not satisfy
the end-to-end governance, durability, recovery, or attachment exit criteria.

## 4. Exit criteria

| ID | Criterion | Required proof |
| --- | --- | --- |
| ACP-01 | Every connection starts with ACP `initialize` using wire version `1` | Exact request/response fixtures and wrong-version rejection |
| ACP-02 | SDK package version and wire version remain independent | Dependency policy test or review plus documentation assertion |
| ACP-03 | First release uses local stdio and does not depend on draft Streamable HTTP | Configuration and transport integration tests |
| ACP-04 | ACP `sessionId` maps only to `AgentSessionId` | Type-level API review and cross-ID negative tests |
| ACP-05 | `TaskId`, `RunId`, and event sequence survive Agent process restart | Durable recovery integration test |
| ACP-06 | Optional ACP methods are called only when advertised | Capability matrix tests |
| ACP-07 | Prompt chunks, plans, tool calls, usage, and stop reasons map deterministically | Golden mapping fixtures |
| ACP-08 | `session/request_permission` always enters Approval and Broker policy | End-to-end fake Agent test and direct-call prohibition review |
| ACP-09 | `fs/*` never performs direct bridge filesystem I/O | Broker fake assertions plus traversal and symlink PoCs |
| ACP-10 | `terminal/*` uses target-bound governed execution handles | Create/output/wait/kill/release lifecycle tests |
| ACP-11 | Cancel settles outstanding prompt, permission, and callback work | Race and timeout tests with no late execution |
| ACP-12 | Malformed or contaminating stdout fails closed; stderr is bounded and redacted | Adversarial subprocess fixtures |
| ACP-13 | Backpressure cannot grow memory without a bound | Saturation test with defined termination result |
| ACP-14 | Load replay and resume-without-replay are distinguishable | Event flags and presentation replay test |
| ACP-15 | Unsupported recovery never silently resends a prompt | Crash/restart test that reaches explicit blocked state |
| ACP-16 | Disabling the ACP runtime profile restores the existing runtime paths | Rollback smoke test |

All criteria are mandatory for Phase 2 exit. Optional ACP features may remain
disabled, but any advertised feature must pass its complete callback and
governance criteria.

## 5. Required test evidence

The implementation acceptance report must record:

- full candidate commit SHA;
- exact ACP SDK crate version from `Cargo.lock`;
- exact targeted test commands and test counts;
- official ACP v1 schema or conformance fixture revision;
- supported capability matrix;
- subprocess limits for line size, stderr, queue depth, and timeouts;
- adversarial proof for path escape, ID confusion, permission spoofing, output
  contamination, duplicate execution, and cancellation races;
- untested optional ACP features and transports.

Current focused command:

```text
cargo +1.88.0 test --package cosh-gateway runtime::acp
```

Result on the uncommitted candidate worktree: the existing ACP codec/driver
suite and five ACP-port tests pass. The latter cover mapping, identity
substitution, one-shot correlation, missing once-only choices, cancellation,
and settlement ordering. This remains first-slice evidence rather than the
full Phase 2 conformance suite. The separate Core-port lifecycle suite is not
ACP conformance evidence.

## 6. Manual and live validation

No provider, ECS, manual terminal, or screenshot validation was requested or
performed for this implementation slice. A future live gate must not be marked
passed until it runs the exact candidate commit and records sanitized evidence.

## 7. Remaining blockers

- Phase 0 Runtime Port, ID, event, persistence, and supervision contracts must
  be accepted first.
- Phase 1 Task Plane, Capability Broker, Approval Service, and Execution Target
  must be available.
- Fixed executable names, local resolution, pinned source installer, and the
  installed entrypoint exist; signed/offline distribution policy remains.
- Output, terminal lifetime, and optional replay policy limits need approved
  values.

## 8. Acceptance decision

**PARTIAL IMPLEMENTATION / NOT ACCEPTED.** The v1 codec and supervised stdio
bridge are real candidate evidence, but Phase 2 acceptance still requires all
ACP-01 through ACP-16 criteria on one candidate revision.
