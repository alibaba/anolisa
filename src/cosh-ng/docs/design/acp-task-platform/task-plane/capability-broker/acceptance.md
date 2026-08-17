# Phase 1 Capability Broker Acceptance Report

[中文版](acceptance_zh.md) | [Design](design.md)

## Result

**Overall: PARTIAL. Generic Broker foundations exist; Phase 1 does not pass.** The
implementation worktree is based on
`e90d9d9402c7fa1c8122267eb4e075c0adda51f5`.

The foundational Broker logic validates Capability request expiry, authoritative Task, Run, complete Actor
provenance, target, operation descriptor, complete operation digest, and requested scope before
policy. It separates policy decisions from permits, issues exactly bound single-use permits, and
atomically consumes them in a process-local memory store. Eight targeted tests pass. These are
generic contract/logic foundations, not production execution evidence.

This result is not a universal governance claim. The production `CoshBrokered`
profile is task-only and exposes `ask_user_question` only. It has no production
`ExecutionTarget` and no checkpoint/ws-ckpt dependency. Generic Capability,
Permit, and Execution contracts/ledger rows remain future foundations. Shell,
Skill, MCP, extension-tool, legacy CLI, and interactive Core mutation paths
remain outside this boundary.

The owner scope decision is therefore: Phase 1 criteria below apply to the enabled Gateway
production profile only. Production `serve` and the library daemon admit only `core` /
`gateway-brokered-v1`. ACP `doctor`/`run` interoperability, standalone Shell, and legacy CLI are
explicitly ungoverned; compatibility evidence from those paths cannot satisfy a Broker criterion.
The formal universal Broker exit remains unmet.

## Durable ledger storage result

**Overall: VERIFIED DURABLE TASK STORAGE SLICE; PRODUCTION BROKER INTEGRATION REMAINS
NOT IMPLEMENTED.** Checksummed SQLite schema v9 stores Task/Run events and projections,
approval/input state, Outbox intents, Runtime bindings, fenced Run leases, and
durable dispatch receipts. Generic permit/execution ledger contracts remain
available as future foundations; this PR does not wire a production target or
claim checkpoint execution, typed side-effect results, or reconciliation.

The durable ledger suite is reproduced with
`cargo test --locked --package cosh-gateway storage --no-fail-fast`. Its fixtures include
stale-lease, cross-Task Run, generation-skip,
approval-deadline widening, integer-overflow, idempotency-namespace, receipt-corruption, and atomic
rollback fixtures.

No checkpoint driver, ws-ckpt resolver, production target, or pre-effect
execution loop is wired. Target resolution, durable permit issuance, audit
gating, execution, and reconciliation remain future optional capability work.

## Result vocabulary

| Result | Meaning |
| --- | --- |
| PASS | Reproducible evidence satisfies the complete criterion for the stated scope. |
| PARTIAL | Implemented evidence satisfies only the explicitly listed subset. |
| FAIL | An enabled current path violates the target invariant. |
| NOT IMPLEMENTED | No implementation exists for the criterion. |
| BLOCKED | A prerequisite decision prevents verification. |

## Implementation evidence

| Source | Verified behavior |
| --- | --- |
| [`capability.rs`](../../../../../crates/cosh-gateway/src/capability.rs) | Exposes the Broker, policy, permit-store, claim, context, and memory-store boundaries without exposing an executor |
| [`broker.rs`](../../../../../crates/cosh-gateway/src/capability/broker.rs) | Validates expiry, Task/Run/full `ActorRef`, and authoritative target/descriptor/full-operation digest/scope before policy; rejects unavailable or invalid policy authority and exposes atomic claim |
| [`memory.rs`](../../../../../crates/cosh-gateway/src/capability/memory.rs) | Holds permit validation and consumption under one mutex; mismatch, expiry, and replay fail closed |
| [`memory/tests.rs`](../../../../../crates/cosh-gateway/src/capability/memory/tests.rs) | Covers parent and actor-provenance substitution, policy branches/failures, permit binding, mismatch, expiry/replay, and concurrent consumption |
| [`capability.rs`](../../../../../crates/cosh-gateway-contracts/src/capability.rs) | Defines neutral request, decision, approval, and permit contracts with Actor/Task/Run/Execution/target/operation/policy/expiry bindings |
| [`scheduler.rs`](../../../../../crates/cosh-gateway/src/daemon/scheduler.rs) | Keeps durable Task/Run/Outbox/lease/input/cancel/retry/recovery coordination separate from future execution-target adapters |

The Broker source depends on contracts and its two explicit ports. It does not import Task storage,
Runtime bridges, OS operators, ACP, or network APIs.

## Acceptance matrix

| ID | Criterion | Result | Evidence or remaining gap |
| --- | --- | --- | --- |
| CBR-001 | Every side effect enabled in the Gateway production profile uses typed `CapabilityRequest`. | PASS for task-only scope | The immutable inventory exposes only side-effect-free `ask_user_question`; no production side effect is enabled. |
| CBR-002 | Its target resolves to an immutable authenticated local identity. | NOT IMPLEMENTED | No production `ExecutionTarget` or target resolver is wired; checkpoint/ws-ckpt identity is future work. |
| CBR-003 | Policy result, approval, and permit are distinct types in that path. | PARTIAL | Generic contracts and in-memory logic distinguish the types; no production side-effect path issues them. |
| CBR-004 | Every permitted effect in that profile has one `ExecutionId`. | NOT IMPLEMENTED | The task-only inventory has no permitted OS effect or production execution target. |
| CBR-005 | Its permit binds actor, Task, Run, target, operation digest, policy, fence, expiry, and one use. | PARTIAL | Generic permit validation covers the binding in logic tests; durable production issuance remains future work. |
| CBR-006 | Its target verifies and consumes the permit immediately before execution. | NOT IMPLEMENTED | No production target is wired, so no target-side consume or execution loop is claimed. |
| CBR-007 | Its approval is durable Task state and cannot widen authority. | PARTIAL | Durable Task approval state exists; approval-to-permit and target binding remain future work. |
| CBR-008 | Broker never writes the Task aggregate. | PASS | Broker has no Task aggregate or storage dependency and returns decisions only. |
| CBR-009 | Repeated execute in the profile cannot produce a second effect. | NOT IMPLEMENTED | No production effect or execution target is enabled in the task-only profile. |
| CBR-010 | Crash uncertainty triggers typed reconciliation, never automatic retry. | PARTIAL | Task/Run restart recovery and fail-closed retry boundaries exist; typed side-effect reconciliation is future work. |
| CBR-011 | No opaque Shell fallback is enabled in the governed profile. | PASS for task-only scope | The accepted inventory contains no Shell or other side-effecting operation. Legacy parser behavior is compatibility evidence only. |
| CBR-012 | Typed policy has allow/deny/require-approval outcomes. | PASS | Neutral `PolicyPort` and deterministic tests cover all three outcomes plus unavailable/invalid authority. |
| CBR-013 | Execution start in the profile requires durable security audit. | NOT IMPLEMENTED | No production execution start or target exists in the task-only profile. |
| CBR-014 | Direct Core side-effecting tools are disabled or delegated in the profile. | PASS for task-only scope | The immutable inventory contains only `ask_user_question`; hooks, MCP, Skills, extensions, Shell, file, process, network, and checkpoint paths are disabled. |
| CBR-015 | Production Gateway operations cannot bypass permit in governed mode. | PASS for task-only scope | `serve` and the library daemon admit only the task-only selector; no production side-effect operation can bypass a permit. ACP `doctor`/`run` and legacy CLI are explicitly outside the governed claim. |
| CBR-016 | Remote identity is disabled until a v2 attestation decision is approved. | PASS for scope decision | Phase 1 is local installation-scoped single-tenant; remote and `TenantId`/multi-tenant support are future v2. |

## Validation evidence

Commands run from `src/cosh-ng`:

```text
cargo fmt --package cosh-gateway -- --check
cargo test --locked --package cosh-gateway-contracts
result: package tests and doc-test targets passed

cargo test --locked --package cosh-gateway capability::
result: targeted capability suite passed

cargo clippy --locked --package cosh-gateway-contracts --all-targets -- -D warnings
cargo clippy --locked --package cosh-gateway --all-targets -- -D warnings
result: passed with zero warnings

cargo doc --locked --package cosh-gateway-contracts --package cosh-gateway --no-deps
result: passed
```

The eight tests prove:

- request expiry and Task, Run, Actor ID, issuer, assurance, target, operation descriptor,
  complete operation digest, and scope substitution fail closed before policy;
- policy deny and approval never create a permit;
- policy unavailability, zero revision, and expired authority fail closed;
- an issued permit binds actor, Task, Run, Execution, target, complete canonical operation digest, policy revision,
  expiry, and one use;
- wrong actor, Task, Run, Execution, target, complete operation digest, or policy revision does not consume
  authority;
- expired and repeated consumption fail closed;
- exactly one of eight simultaneous claims succeeds.

Generic contract/ledger and scheduler suites were exercised. No checkpoint
adapter/driver, ws-ckpt resolver, production target, or side-effect execution
loop is claimed. The destructive packaged-unit containment fixture passed on
disposable Ubuntu 24.04 arm64/systemd 255. No
accepted real Codex/Claude, manual Terminal, ECS, network, or universal tool
validation is claimed.

## Required remaining artifacts

| Artifact | Required proof |
| --- | --- |
| Universal approval and re-authorization tests | Every presentation path can issue authority only from a committed matching approval. |
| Immutable target-substitution matrix | Workspace, UID, boot, container, and instance changes invalidate permits. |
| Multi-operation permit/execution matrix | The generic invariants hold for every future governed operation. |
| Universal security audit gate | Issuance and execution start fail when required audit persistence fails for every target. |
| Execution kill-point and reconciliation matrix | Claimed, started, and uncertain effects never auto-replay. |
| Broker bypass inventory | Every enabled Gateway/Core/Shell/ACP/Skill/MCP effect reaches the verifier. |
| Revocation and lease-fence corpus | Revoked, stale-runtime, and stale-policy authority fails closed. |
| Trusted canonicalizer tests | Independent canonicalization binds descriptor and digest before Broker admission. |

## Exit criteria

The task-only production profile satisfies only the bounded inventory decision,
but Phase 1 remains PARTIAL because production target execution is not
implemented and the recorded real-provider/manual release gates remain open. A
formal universal Broker exit additionally requires:

1. Approval resolution, permit issuance, durable consumption, audit, execution, and typed
   reconciliation for every future enabled operation to form one reviewed security boundary.
2. Universal immutable target identity, revocation, and remote attestation decisions.
3. A bypass inventory covering every future enabled Gateway/Core/Shell/ACP/Skill/MCP mutation edge.
4. Crash, replay, substitution, audit-failure, revocation, real-provider, and manual fixtures on one
   exact release candidate.

## Remaining risks

- `MemoryPermitStore` remains test-only; production authority for any future
  target path is not yet wired.
- Target identity and execution coverage, including checkpoint/ws-ckpt, remain
  future optional capabilities rather than current production evidence.
- Ungoverned Shell, ACP interoperability, Skill, MCP, extension, and legacy effects remain outside
  the closed production profile and must never be advertised as governed.
