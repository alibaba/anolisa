# Phase 1 Capability Broker Acceptance Report

[中文版](acceptance_zh.md) | [Design](design.md)

## Result

**Overall: PARTIAL. Generic Broker foundations exist, but no checkpoint execution target or
ledger-side reconciliation ships; Phase 1 does not pass.** The implementation worktree is based on
`a43ab81738d3f39721a425cef717a6147276fae9`.

The foundational Broker logic validates Capability request expiry, authoritative Task, Run, complete Actor
provenance, target, operation descriptor, complete operation digest, and requested scope before
policy. It separates policy decisions from permits, issues exactly bound single-use permits, and
atomically consumes them in a process-local memory store. Targeted capability tests pass. These are
generic contract/logic foundations, not production execution evidence.

This result is not a universal governance claim. The production `CoshBrokered`
profile is bound to the pinned `task-only-v1` manifest and exposes
`ask_user_question` only. The daemon, durable start intent, installed Core
factory, and private v3 handshake verify the identity before execution or Task
input, and that profile has no checkpoint/ws-ckpt dependency. Generic Capability,
Permit, and Execution contracts/ledger rows remain future foundations. Shell,
Skill, MCP, extension-tool, legacy CLI, and interactive Core mutation paths
remain outside this boundary.

## Optional profile and deferred checkpoint target result

**Overall: CLOSED PROFILE AND SEALED PROVIDER SET VERIFIED; CHECKPOINT PROVIDER AUTHORITY WITHHELD;
CHECKPOINT EXECUTION TARGET DEFERRED.** The Gateway owns a closed `workspace-checkpoint-v1` profile with
a pinned canonical manifest and digest, a sealed one-provider set, and a `SealedCapabilityProviderRegistry`
that refuses every requested checkpoint provider. `CkptClient` additionally gained effect classification, a
read-only evidence query, and peer authentication.

**No checkpoint execution target exists in this increment.** A `workspace_checkpoint_create` permit must
authorize exactly one snapshot creation, and the ws-ckpt checkpoint request cannot be constrained to that:
dispatch unconditionally runs workspace auto-initialization first, and a workspace identity whose
registration disappeared between any pre-request query and the request is resolved as a relative path.
Auto-initialization registers a workspace, can adopt a subvolume, moves a directory aside, creates a
symlink, and removes a broken symlink. A checkpoint-create permit grants none of that; the Gateway cannot
prevent it, because the query and the request are separate round trips, and cannot undo it. The target is
therefore deferred rather than shipped behind a gate, which also keeps `cosh-gateway` depending only on the
side-effect-free `cosh-gateway-contracts` leaf among internal crates.

Two ws-ckpt protocol prerequisites are recorded for the deferred slice: a checkpoint request that resolves
a workspace identity strictly and never auto-initializes, and a non-reusable workspace generation token
validated atomically with checkpoint creation. The design document additionally records the socket-trust,
workspace-representation, btrfs volume-identity, and generation constraints established while prototyping,
so they do not have to be rediscovered.

`serve`, the packaged systemd unit, the installed Core factory, the private Core v3 mirror, and the
brokered execution driver are unchanged, so no Runtime can request or obtain a checkpoint. A
checkpoint-enabled instance is not startable end to end, and no release may advertise governed checkpoints.

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

No checkpoint driver or pre-effect execution loop is wired to the durable ledger.
Durable permit issuance, audit gating, and Runtime-visible checkpoint execution
remain future work. No trusted configuration admits a checkpoint execution
target, and no Task can reach one.

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
| [`profile.rs`](../../../../../crates/cosh-gateway-contracts/src/profile.rs) | Pins the `task-only-v1` and `workspace-checkpoint-v1` identities, canonical manifest digests, governed targets, exact Runtime inventories, and sealed provider sets |
| [`provider.rs`](../../../../../crates/cosh-gateway/src/capability/provider.rs) | Admits exactly the provider set a profile seals; rejects a requested provider on a Task-only instance and a missing provider on a checkpoint-enabled instance, and withholds the checkpoint provider for every profile |
| [`checkpoint.rs`](../../../../../crates/cosh-platform/src/checkpoint.rs) | Authenticates the connected peer before any write, returns proven no-effect or `PossiblyApplied` according to the request phase, and provides an exact read-only evidence query. No Gateway caller exists yet; these are transport primitives for a future target and ledger-side reconciler |
| [`scheduler.rs`](../../../../../crates/cosh-gateway/src/daemon/scheduler.rs) | Keeps durable Task/Run/Outbox/lease/input/cancel/retry/recovery coordination separate from future execution-target adapters |

The Broker source depends on contracts and its two explicit ports. It does not import Task storage,
Runtime bridges, OS operators, ACP, or network APIs.

## Acceptance matrix

| ID | Criterion | Result | Evidence or remaining gap |
| --- | --- | --- | --- |
| CBR-001 | Every side effect enabled in the Gateway production profile uses typed `CapabilityRequest`. | PASS for task-only scope | The immutable inventory exposes only side-effect-free `ask_user_question`; no production side effect is enabled. |
| CBR-002 | Its target resolves to an immutable authenticated local identity. | NOT IMPLEMENTED | No checkpoint execution target exists. The identity requirements established while prototyping — socket path chain plus peer authentication, both workspace representations, and a `(filesystem ID, subvolume ID, inode)` btrfs volume identity — are recorded in the design document as prerequisites for the deferred slice. |
| CBR-003 | Policy result, approval, and permit are distinct types in that path. | PARTIAL | Generic contracts and in-memory logic distinguish the types; no production side-effect path issues them. |
| CBR-004 | Every permitted effect in that profile has one `ExecutionId`. | NOT IMPLEMENTED | The task-only inventory has no permitted OS effect or production execution target. |
| CBR-005 | Its permit binds actor, Task, Run, target, operation digest, policy, fence, expiry, and one use. | PARTIAL | Generic permit validation covers the binding in logic tests; durable production issuance remains future work. |
| CBR-006 | Its target verifies and consumes the permit immediately before execution. | NOT IMPLEMENTED | No production target exists, so no target-side consume or execution loop is claimed. |
| CBR-007 | Its approval is durable Task state and cannot widen authority. | PARTIAL | Durable Task approval state exists; approval-to-permit and target binding remain future work. |
| CBR-008 | Broker never writes the Task aggregate. | PASS | Broker has no Task aggregate or storage dependency and returns decisions only. |
| CBR-009 | Repeated execute in the profile cannot produce a second effect. | PARTIAL | No admitted provider enables an effect. The transport supplies a read-only evidence primitive that a future target could use instead of issuing a second create after a lost response, but no execution target or permit loop exists. |
| CBR-010 | Crash uncertainty triggers typed reconciliation, never automatic retry. | PARTIAL | Task/Run restart recovery and fail-closed retry boundaries exist. After the first request byte, the checkpoint transport returns `PossiblyApplied` for every failure, including a daemon-reported error code, and exposes a read-only evidence primitive. Target-side and ledger-side reconciliation are future work. |
| CBR-011 | No opaque Shell fallback is enabled in the governed profile. | PASS for task-only scope | The accepted inventory contains no Shell or other side-effecting operation. Legacy parser behavior is compatibility evidence only. |
| CBR-012 | Typed policy has allow/deny/require-approval outcomes. | PASS | Neutral `PolicyPort` and deterministic tests cover all three outcomes plus unavailable/invalid authority. |
| CBR-013 | Execution start in the profile requires durable security audit. | NOT IMPLEMENTED | No production execution start or target exists in the task-only profile. |
| CBR-014 | Direct Core side-effecting tools are disabled or delegated in the profile. | PASS for task-only scope | The immutable inventory contains only `ask_user_question`; hooks, MCP, Skills, extensions, Shell, file, process, network, and checkpoint paths are disabled. |
| CBR-015 | Production Gateway operations cannot bypass permit in governed mode. | PASS for task-only scope | `serve` and the library daemon derive the target from the pinned profile; Runtime start schema v3 and the Core handshake reject identity or inventory drift before launch/input. No production side-effect operation can bypass a permit. ACP `doctor`/`run` and legacy CLI are explicitly outside the governed claim. |
| CBR-016 | Remote identity is disabled until a v2 attestation decision is approved. | PASS for scope decision | Phase 1 is local installation-scoped single-tenant; remote and `TenantId`/multi-tenant support are future v2. |

## Validation evidence

Commands run from `src/cosh-ng` on the rebased candidate:

```text
cargo fmt --all -- --check
cargo test --locked -p cosh-gateway-contracts profile::
cargo test --locked -p cosh-platform checkpoint::
cargo test --locked -p cosh-gateway
cargo clippy --locked -p cosh-gateway-contracts -p cosh-platform -p cosh-gateway --all-targets -- -D warnings
cargo doc --locked --no-deps -p cosh-gateway-contracts -p cosh-gateway
```

The targeted tests prove:

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

The optional profile and withheld provider tests additionally prove:

- the `task-only-v1` canonical manifest and pinned digest are unchanged and contain no provider
  section, so the private Core v3 mirror still verifies the same identity;
- the second profile's manifest, digest, exact two-tool inventory, and one-provider set reject every
  missing, additional, reordered, renamed, or substituted value, including the rejected `ws-ckpt-v1`
  name;
- a Task-only instance admits an empty provider set with no ws-ckpt socket, directory, or daemon
  present, and rejects a configured checkpoint provider instead of widening;
- a checkpoint-enabled instance refuses admission when no provider is requested;
- the checkpoint provider is withheld for the profile that seals it, and an exhaustive test over every
  profile and requested-provider combination shows no admission outcome yields checkpoint side-effect
  authority;
- every transport phase returns proven no-effect or `PossiblyApplied`, including all thirteen daemon
  error codes, plus peer authentication and the exact read-only evidence query.

Generic contract/ledger and scheduler suites were exercised. The destructive
packaged-unit containment fixture passed on disposable Ubuntu 24.04
arm64/systemd 255. Checkpoint evidence comes from a fake Unix daemon only; no
real ws-ckpt daemon, accepted real Codex/Claude, manual Terminal, ECS, network,
or universal tool validation is claimed.

No btrfs behaviour is validated, because no code in this increment reads it. The
btrfs volume-identity requirement is recorded in the design document rather than
implemented, and the prototype that established it is preserved outside the
delivery branch. Validating it needs a privileged btrfs environment and belongs to
the deferred slice.

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
- No checkpoint execution target is implemented or Runtime-reachable. The
  checkpoint profile and withheld provider must not be read as governed
  checkpoint support.
- The transport supplies only a read-only, workspace-scoped evidence primitive.
  A future ledger-side reconciler would need a narrow exact-query protocol if
  that evidence becomes ambiguous or unbounded before recording a conclusive
  outcome.
- No ws-ckpt response code is accepted as pre-effect evidence, so an ordinary
  daemon rejection makes the transport return `PossiblyApplied`. No durable
  uncertain receipt is recorded today; a future target and ledger would have to
  persist that uncertainty until reconciliation. Reducing it requires an explicit
  pre-effect guarantee in the protocol, not a classification table maintained
  against daemon internals.
- The daemon remains the trust anchor for its own workspace registry. A
  compromised daemon is outside this boundary.
- **Checkpoint requests are not identity-only.** ws-ckpt runs workspace
  auto-initialization before every checkpoint, and resolves an unregistered
  workspace identity as a relative path, so a checkpoint-create permit could cause
  workspace registration or symlink removal outside its authority. Provider
  admission is withheld and the execution target is deferred until the protocol
  offers a strict identity-only request. This is an authority gap, not a reporting
  gap, and no Gateway-side check closes it.
- **A Gateway-side checkpoint transport has no admitted home.** Among internal
  crates `cosh-gateway` depends only on `cosh-gateway-contracts`. Reusing
  `CkptClient` from a Gateway target would add `cosh-platform` and `cosh-types`
  edges, so the deferred slice must first decide where that transport may live.
- **Generation attribution is not fenced.** The ws-ckpt workspace identity is
  derived from the workspace path and is reusable after an unregister, and
  `rollback` to the current DAG head replaces the live subvolume while leaving the
  workspace ID, the registration path, and `index.head` unchanged. A future target
  would need to compare the registered path, workspace volume identity, and daemon
  mapping against admission both before and after every request. The prototype
  established that a btrfs filesystem identifier plus the non-reused subvolume ID
  would detect a rollback that persists through either comparison, while a rollback
  confined entirely to the create window would remain invisible because no protocol
  value can be validated atomically. A future ledger therefore must not bind a
  conclusive receipt to the workspace-content generation until the ws-ckpt protocol
  provides a non-reusable workspace generation token validated atomically with
  checkpoint creation; the same prerequisite applies to any immutable workspace
  fence claim.
- Ungoverned Shell, ACP interoperability, Skill, MCP, extension, and legacy effects remain outside
  the closed production profile and must never be advertised as governed.
