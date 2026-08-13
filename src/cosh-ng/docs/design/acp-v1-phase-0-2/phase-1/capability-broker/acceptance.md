# Phase 1 Capability Broker Acceptance Report

[中文版](acceptance_zh.md) | [Design](design.md)

## Result

**Overall: PARTIAL. The process-local broker logic slice passes; Phase 1 does not.** The
implementation worktree is based on
`6c115aefe04ace0d169a24fa7cd55ad7c1befa52`.

The Gateway now validates Capability request expiry, authoritative Task, Run, complete Actor
provenance, target, operation descriptor, complete operation digest, and requested scope before
policy. It separates policy decisions from permits, issues exactly bound single-use permits, and
atomically consumes them in a process-local memory store. Eight targeted tests pass.

This result is not an end-to-end governance claim. `MemoryPermitStore` is non-durable. Approval
resolution and re-authorization are absent. There is no immutable target resolver, lease/runtime
fence, durable permit/execution ledger, audit gate, OS executor, revocation, crash recovery,
reconciliation, network API, or ACP integration. Existing CLI/Core/Shell mutation paths still
bypass this slice.

## Durable ledger storage result

**Overall: VERIFIED DURABLE STORAGE SLICE; BROKER INTEGRATION REMAINS PARTIAL.** A checksummed
SQLite v2 migration now stores approval resolution, single-use permits, execution state and
receipts, Runtime bindings, and fenced Run leases. The durable consume path replays the
authoritative Task event stream, requires the exact current lease claim, atomically changes an
issued permit and planned execution to consumed/started, and marks an unreceipted started execution
`uncertain` during recovery without retrying the effect. Approved permits cannot outlive or widen
their approval bindings.

`cargo +1.88.0 test --locked --package cosh-gateway storage --no-fail-fast` passed 28/28 targeted
storage tests on 2026-08-13. These tests include stale-lease, cross-Task Run, generation-skip,
approval-deadline widening, integer-overflow, idempotency-namespace, receipt-corruption, and atomic
rollback fixtures.

The process-local `CapabilityBroker` is not yet wired to this store, so existing bypass findings,
immutable target resolution, required audit persistence, the OS executor, and reconciliation remain
open.

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

The Broker source depends on contracts and its two explicit ports. It does not import Task storage,
Runtime bridges, OS operators, ACP, or network APIs.

## Acceptance matrix

| ID | Criterion | Result | Evidence or remaining gap |
| --- | --- | --- | --- |
| CBR-001 | Every side-effect request uses typed `CapabilityRequest`. | FAIL | Broker input is typed, but existing CLI/Core/Shell mutation paths bypass it. |
| CBR-002 | Target resolves to an immutable authenticated identity. | NOT IMPLEMENTED | The first slice binds exact `TargetRef`; there is no resolver, boot/workspace/UID identity, or attestation. |
| CBR-003 | Policy result, approval, and permit are distinct types. | PARTIAL | `PolicyDecision`, `ApprovalRequest`, `CapabilityDecision`, and `ExecutionPermit` are distinct; durable approval resolution/re-authorization is absent. |
| CBR-004 | Every permitted effect has one `ExecutionId`. | PARTIAL | Every Broker-issued permit gets one `ExecutionId`; bypass paths and execution lifecycle are not integrated. |
| CBR-005 | Permit binds actor, Task, Run, target, operation digest, policy, fence, expiry, and one use. | PARTIAL | Durable consume verifies exact actor/Task/authoritative Run/Execution/target/digests/policy/expiry plus current lease generation and revision. Immutable target identity and production Broker wiring remain. |
| CBR-006 | Target verifies and consumes permit immediately before execution. | PARTIAL | Durable consume and execution start are one transaction, but no OS target adapter invokes the ledger immediately before an effect. |
| CBR-007 | Approval is durable Task state and cannot widen authority. | PARTIAL | Durable approval resolution and exact approval-bound issuance exist; the approval ledger is not yet atomically integrated with Task approval events. Direct policy permits remain supported. |
| CBR-008 | Broker never writes the Task aggregate. | PASS | Broker has no Task aggregate or storage dependency and returns decisions only. |
| CBR-009 | Repeated execute cannot produce a second effect. | PARTIAL | Durable single-use consume and idempotent command receipts survive restart; executor integration must still prove that `Replayed` outcomes never repeat an effect. |
| CBR-010 | Crash uncertainty triggers typed reconciliation, never automatic retry. | PARTIAL | Recovery changes every unreceipted started execution to `uncertain` without retry; a typed reconciliation port is absent. |
| CBR-011 | Shell parsing fails closed on adversarial separators and metacharacters. | PASS | Existing parser/heuristic baseline remains reusable; the new Broker does not add a Shell fallback. |
| CBR-012 | Typed policy has allow/deny/require-approval outcomes. | PASS | Neutral `PolicyPort` and deterministic tests cover all three outcomes plus unavailable/invalid authority. |
| CBR-013 | Permit issuance and execution start require durable security audit. | NOT IMPLEMENTED | The memory store has no audit port or durable issuance gate. |
| CBR-014 | cosh-core direct side-effecting tools are disabled/delegated in brokered mode. | FAIL | Brokered Core integration does not exist. |
| CBR-015 | CLI/platform operations cannot bypass permit in governed mode. | FAIL | No governed operator integration exists. |
| CBR-016 | Canonical remote target identity/attestation is approved. | BLOCKED | The target identity decision and implementation remain open. |

## Validation evidence

Commands run from `src/cosh-ng`:

```text
cargo fmt --package cosh-gateway -- --check
cargo test --locked --package cosh-gateway-contracts
result: 6 integration tests passed; unit and doc-test targets passed

cargo test --locked --package cosh-gateway capability::
result: 8 passed; 0 failed; 38 filtered out

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

No ECS, provider, OS mutation, network, or ACP validation was run because this slice deliberately
contains no such adapter.

## Required remaining artifacts

| Artifact | Required proof |
| --- | --- |
| Durable approval and re-authorization tests | Only a committed, matching approval can issue a no-wider permit. |
| Immutable target-substitution matrix | Workspace, UID, boot, container, and instance changes invalidate permits. |
| Durable permit/execution ledger | Restart cannot restore consumed authority or duplicate a known/unknown effect. |
| Security audit gate | Issuance and execution start fail when required audit persistence fails. |
| Execution kill-point and reconciliation matrix | Claimed, started, and uncertain effects never auto-replay. |
| Broker bypass inventory | Every enabled Gateway/Core/Shell/ACP/Skill/MCP effect reaches the verifier. |
| Revocation and lease-fence corpus | Revoked, stale-runtime, and stale-policy authority fails closed. |
| Trusted canonicalizer tests | Independent canonicalization binds descriptor and digest before Broker admission. |

## Exit criteria

1. CBR-001 through CBR-015 are PASS; remote execution remains disabled until CBR-016 passes.
2. Approval resolution, permit issuance, durable consumption, audit, execution, and reconciliation
   are one reviewed security boundary.
3. Immutable target identity and runtime/lease fencing replace the initial `TargetRef` binding.
4. The bypass inventory covers every enabled mutation edge with no direct executor path.
5. Crash, replay, substitution, audit-failure, and revocation fixtures pass on one exact commit.

## Remaining risks

- Process restart erases `MemoryPermitStore`, so it cannot protect production authority.
- An unresolved approval cannot authorize a permit; adding that path without durable matching
  would create an authority-widening vulnerability.
- Exact `TargetRef` equality does not detect boot, UID, namespace, symlink, or workspace changes.
- A caller can still bypass the Broker through current Core, CLI, and platform execution paths.
- Claiming a permit without a durable execution state cannot reconcile a crash after an effect.
