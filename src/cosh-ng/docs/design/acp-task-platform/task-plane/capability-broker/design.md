# Phase 1 Capability Broker Design

[中文版](design_zh.md) | [Acceptance report](acceptance.md)

## Status and decision

This increment is based on upstream commit `a43ab817`. Its universal Broker model remains the
architecture target, not an accepted Phase 1 claim. The accepted production scope is narrower:
`serve` and the library daemon admit only `core` / `gateway-brokered-v1`, whose immutable inventory
is bound to the pinned `task-only-v1` manifest and contains `ask_user_question` only. Gateway,
durable Runtime start intent, and Core v3 negotiation verify that identity and exact inventory
before launch or Task input. Every side-effecting hook, Skill, MCP, extension,
Shell, file, process, and network path is disabled in this profile. ACP `doctor`/`run`, legacy CLI
commands, and standalone Shell are explicitly ungoverned interoperability/rollback paths and
cannot be cited as governed evidence.

The Gateway now additionally owns an optional `workspace-checkpoint-v1` capability profile and a sealed
capability provider registry that **withholds** the checkpoint provider for every profile. **No checkpoint
execution target exists**: it is deferred until the ws-ckpt protocol offers an identity-only checkpoint
request and a non-reusable workspace generation token, because without them a checkpoint-create permit
could make the daemon mutate the host outside the authority it grants. `serve`, the packaged systemd unit,
and the brokered execution driver are unchanged as well. The accepted claim is "the closed profile and the
sealed provider set exist and fail closed, and the checkpoint transport classifies effects correctly", not
"a ws-ckpt target exists" and not "checkpoints are governed end to end".

The universal target still requires `CapabilityBroker` as the mandatory policy enforcement and
permit authority for every enabled OS side effect, regardless of origin. An execution target
accepts work only with a valid, target-bound, operation-bound permit.

Approval is necessary when policy requests it but is never itself executable authority. A
committed approval may authorize the Broker to issue a narrower permit; it cannot widen actor,
target, action, resource, lifetime, or execution count.

## Implemented generic foundation (not production execution)

The Gateway now contains a provider-neutral
[`PolicyPort`](../../../../../crates/cosh-gateway/src/capability/broker.rs),
[`PermitStore`](../../../../../crates/cosh-gateway/src/capability/broker.rs), and
`CapabilityBroker`. Authorization validates request expiry, exact Task and Run parents, the
complete authenticated `ActorRef`, and an `AuthoritativeRequestBinding` before consulting policy.
The binding pins exact target, full `OperationDescriptor`, complete operation digest, and requested
scope. Actor provenance or request-content substitution therefore cannot influence policy. Policy
can deny, require approval, or allow. Zero policy revision and already-expired policy authority
fail closed.

Direct allowance issues an `ExecutionPermit` bound to actor, Task, Run, Execution, exact
`TargetRef`, complete canonical operation digest, policy revision, expiry, and one use. The
operation digest covers namespace, name, and normalized arguments; `arguments_digest` remains
available only as narrower policy detail. A trusted ingress owns canonicalization and hashing.
The Broker never derives authority from the argument-only digest. The process-local
[`MemoryPermitStore`](../../../../../crates/cosh-gateway/src/capability/memory.rs) validates those
fields and marks a permit consumed under one mutex, so exactly one concurrent caller can claim it.
A failed binding check does not consume authority.

`MemoryPermitStore` remains a process-local logic fixture and is not production authority. Generic
durable approval/permit/execution ledger contracts may be retained as reusable foundations. The
task-only production profile has no side-effecting operation beyond `ask_user_question`; it does not
invoke a checkpoint provider and has no ws-ckpt dependency.

Phase 1 is installation-scoped and single-tenant. `InstallationId` plus authenticated local peer
credentials form the v1 boundary. `TenantId`, remote peers, and cross-tenant isolation are future
v2 work and are not claimed here.

## Optional sealed provider set and the deferred checkpoint target

A capability profile seals the exact ordered set of side-effect providers an instance may reach.
[`GatewayCapabilityProfile::providers`](../../../../../crates/cosh-gateway-contracts/src/profile.rs)
returns an empty set for `task-only-v1` and exactly `ws-ckpt` for `workspace-checkpoint-v1`, and
`verify_providers` rejects a missing, additional, reordered, or substituted provider. A host that
happens to run the ws-ckpt daemon is therefore never authority on its own.

[`SealedCapabilityProviderRegistry`](../../../../../crates/cosh-gateway/src/capability/provider.rs)
is the single boundary at which an instance would gain side-effect authority. A Task-only instance that
requests a provider is rejected rather than widened, and the sealed set is verified before anything else.

That boundary **withholds** the checkpoint provider for every profile, so the only shape an instance can
reach is the empty provider set. **No checkpoint execution target exists in this crate.** The target is
deferred, not merely unwired: the constraints below are prerequisites any future implementation must
satisfy, and two of them require ws-ckpt protocol changes that no Gateway-side code can substitute for.

Keeping `cosh-gateway` free of a checkpoint adapter also preserves its dependency direction. Among
internal crates it depends only on the side-effect-free `cosh-gateway-contracts` leaf. A Gateway-side
target that reused the existing `CkptClient` would add `cosh-platform` and `cosh-types` edges, so the
deferred slice must first decide where a checkpoint transport may live.

### Why checkpoint authority is withheld

A `workspace_checkpoint_create` permit must authorize exactly one snapshot creation on one admitted
workspace. The ws-ckpt checkpoint request cannot be constrained to that.

Checkpoint dispatch unconditionally runs workspace auto-initialization before it attempts a snapshot.
Auto-initialization resolves the request's workspace field, and that resolution falls through to treating
the value as a relative path when no registration matches it. Workspace initialization is a real host
mutation: it registers a workspace, can adopt an existing subvolume, moves the original directory aside,
creates a symlink, and removes a broken symlink before reporting an invalid path.

Naming the daemon-owned workspace identity instead of a pathname removes the ordinary trigger, because a
registered identity resolves without auto-initialization. It does not remove the window. Any pre-request
identity query and the checkpoint request are separate round trips, so a `recover` that unregisters the
workspace in between leaves the daemon resolving a stale identity string as a relative path. What it then
touches depends on the daemon's working directory, which the Gateway neither controls nor verifies.

No Gateway-side check closes this. A post-request comparison can only downgrade the reported outcome to
indeterminate; it cannot prevent or undo a registration the daemon already performed. Bounding the window
is not a security property either.

**Prerequisite.** A ws-ckpt checkpoint request that resolves a workspace identity strictly and never
auto-initializes.

### Established constraints for the deferred target

These were derived while prototyping the target and are recorded so the deferred slice does not rediscover
them. Each is a requirement, not a description of shipped behaviour.

**Socket trust cannot rest on path metadata.** The daemon publishes a world-connectable socket on purpose,
so the socket's mode is not evidence. Trust requires the socket owner to be root or the Gateway owner and
**every** ancestor directory up to the root to be owned by root or the Gateway owner and not writable by
other principals unless it carries the sticky bit; checking only the immediate parent leaves that parent's
own directory entry replaceable. Even then, path checks cannot close the window between a check and
`connect`. The connected peer must be authenticated with kernel credentials after connecting and before
any request byte is written. Pinning the socket device, inode, and owner remains useful for detecting
replacement, but authentication is what secures the request.

**A workspace has two representations that are not the same string.** After initialization the daemon moves
the original directory into its backend and leaves a symlink at the user-facing path, then keeps reporting
that registered path in its registry, `Status`, and `List`. A descriptor-pinned open of the same path
resolves through the symlink to the backend directory. Requiring the two to be equal rejects every normally
initialized workspace, so both must be bound: the registered path verbatim for daemon queries and evidence
comparison, and the pinned directory for local identity.

**Device and inode do not identify a btrfs subvolume.** Every subvolume root carries inode 256, and a
subvolume's anonymous device number is allocated at first access and may be reused once the subvolume is
deleted. Rollback renames the live subvolume aside, creates a new writable snapshot at the same path, and
deletes the old one, so device and inode can compare equal across two different subvolumes. The containing
subvolume ID from `BTRFS_IOC_INO_LOOKUP` comes from a persistent increasing counter and is not reused, but
it is unique only inside one filesystem, so the filesystem identifier from `BTRFS_IOC_FS_INFO` must scope
it. The identity is therefore `(filesystem ID, subvolume ID, inode)`, with the scheme tag bound so a
workspace cannot silently move to a device-and-inode identity.

**Workspace identity is not a generation fence.** The daemon derives its workspace ID from the workspace
path, so it is reused when the same path is registered again after an unregister, and `rollback` to the
current DAG head replaces the live subvolume while leaving the workspace ID, the registration path, and
`index.head` unchanged. Comparing the volume identity before and after a request detects every rollback
that persists past either comparison, but a rollback confined entirely to the create window is invisible,
and no observable protocol value could be validated atomically with creation. A conclusive receipt could
therefore attest the registered workspace, the checkpoint identity, and the reported snapshot — never the
generation of the workspace contents.

**Prerequisite.** A non-reusable workspace generation token in the ws-ckpt protocol, validated atomically
with checkpoint creation, before any checkpoint receipt may bind workspace contents.

### Effect classification and reconciliation

The transport primitives for this are implemented in
[`CkptClient`](../../../../../crates/cosh-platform/src/checkpoint.rs) and are the one part of the
checkpoint work that ships in this increment, because they are self-contained and correct independently of
the target.

The governed primitives `create_classified` and `find_snapshot` structurally require peer
authentication: the caller must configure `require_trusted_peer(owner_uid)`. Without that configuration,
both reject before inspecting or accessing the socket. `create_classified` reports this rejection as
`KnownNoEffect`, while `find_snapshot` returns a permission error. This fail-closed requirement is scoped
to these two governed primitives; legacy `CkptClient` operations retain their existing optional-peer-auth
behavior.

`create_classified` reports whether a failed request may already have changed daemon state. Only failures
raised strictly before any request byte reaches the kernel are proven `KnownNoEffect`: missing trusted-peer
configuration, a missing socket, a refused connection, an untrusted peer, or a write that transferred
nothing. Everything after that is `PossiblyApplied` — a partial write, a lost or truncated response, an
undecodable payload, an unexpected response variant, **and every daemon-reported error code**.

No response code is treated as pre-effect evidence. The checkpoint dispatch path auto-initializes the
workspace before it attempts a snapshot, so registration, subvolume adoption, or removal of a broken
workspace symlink may already have happened before a code such as `WriteLockConflict`,
`SnapshotAlreadyExists`, or `InvalidPath` is produced. Those codes prove that no snapshot was created; they
do not prove that daemon state is unchanged.

`find_snapshot` is the read-only evidence primitive: it matches one workspace and one exact checkpoint
identity and reports whether the daemon still lists the snapshot as present. A future target and ledger-side
reconciler can use that evidence after a `PossiblyApplied` failure instead of creating a second snapshot,
but this increment does not turn it into a conclusive or durable uncertain outcome.

## Goals

- Normalize all side-effect intent into one typed `CapabilityRequest`.
- Evaluate actor, Task, Run, target, operation, resource scope, risk, and policy revision together.
- Return a stable denial, a durable approval specification, or a short-lived target-bound permit.
- Bind each permitted effect to one `ExecutionId` and a replay-safe execution ledger.
- Make opaque Shell commands fail closed and prefer deterministic typed operators.
- Correlate Task control events with the existing unified security audit contract.
- Preserve a safe local/offline policy path without weakening target or approval checks.

## Non-goals

- Owning the Task aggregate, channel approval UI, Agent lifecycle, PTY rendering, or provider
  session.
- Treating `cosh audit check`, an approval callback, a model tool name, or a policy decision as a
  permit.
- Guaranteeing exactly-once effects across a process or machine crash.
- Granting broad shell, root, filesystem, or network access because a caller is local.
- Supporting remote attestation before the Phase 0 target identity decision is accepted.
- Parsing arbitrary natural language into OS authority.

## Current-source evidence

| Evidence at `6c115aef` | Reusable behavior | Security gap |
| --- | --- | --- |
| [`cosh-types/audit/event.rs`](../../../../../crates/cosh-types/src/audit/event.rs) | Typed `Action`, policy `Decision`, audit identity, versioned event, and redaction shapes. | No capability request, Execution ID, target identity, or permit. |
| [`cosh-platform/audit/evaluate.rs`](../../../../../crates/cosh-platform/src/audit/evaluate.rs) | Deterministic first-match PDP with allow/deny/require-approval. | A policy result is not execution authority. |
| [`cosh-platform/audit/action.rs`](../../../../../crates/cosh-platform/src/audit/action.rs) | Shell action parsing rejects unsupported compound/metacharacter shapes. | Coverage is command-policy classification, not complete target binding. |
| [`cosh-platform/audit.rs`](../../../../../crates/cosh-platform/src/audit.rs) | Policy checks and security-boundary audit segment writes exist. | There is no permit ledger or consume protocol. |
| [`cosh-core/core.rs`](../../../../../crates/cosh-core/src/core.rs) | Hook and policy decisions, approvals, audit events, and tool execution are integrated. | Allowed tools execute inside core; approval does not pass through a common Broker. |
| [`cosh-core/protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs) | `can_use_tool` includes tool, input, tool-use ID, hook flag, and audit reference. | It lacks Task/Run/target/Execution IDs and a permit. |
| [`cosh-platform/pkg.rs`](../../../../../crates/cosh-platform/src/pkg.rs), [`svc.rs`](../../../../../crates/cosh-platform/src/svc.rs), and [`checkpoint.rs`](../../../../../crates/cosh-platform/src/checkpoint.rs) | Typed OS operations and some dry-run paths exist. | Callers invoke them without a shared permit verifier. |

At baseline, `cosh-cli`, `cosh-core`, and Shell execution paths can reach side effects without a
target-bound Broker permit. The existing policy and audit modules are foundations, not evidence
that the Broker already exists.

## Ownership and ports

```mermaid
flowchart LR
    AR["AgentRuntimePort"] --> BP["CapabilityBrokerPort"]
    GW["Gateway direct operation"] --> BP
    BP --> BR["CapabilityBroker"]
    BR --> PDP["PolicyDecisionPort"]
    BR --> AP["ApprovalReadPort"]
    BR --> PL[("Permit / execution ledger")]
    BR --> AU["AuditPort"]
    BR --> ET["ExecutionTargetPort\nfuture optional capability"]
    ET --> V["PermitVerifier"]
    V --> OP["Typed operator / Shell executor"]
    OP --> OS["Bound GuestOS target"]
    BR --> TR["BrokerResultPort"]
    TR --> TC["TaskCoordinator\nsole Task writer"]
```

The Broker owns capability normalization, policy orchestration, permit issuance, permit
consumption state, and execution correlation. Execution adapters own the last-mile operation and
must verify the permit immediately before use. `TaskCoordinator` owns approval state and Task
events; the Broker submits results but never writes Task storage.

Conceptual ports are:

```rust
trait CapabilityBrokerPort {
    async fn authorize(&self, request: CapabilityRequest)
        -> Result<CapabilityDecision, BrokerError>;
    async fn execute(&self, request: PermittedExecution)
        -> Result<ExecutionReceipt, BrokerError>;
    async fn reconcile(&self, execution_id: ExecutionId)
        -> Result<ExecutionStatus, BrokerError>;
}

trait PolicyDecisionPort {
    async fn evaluate(&self, action: PolicyAction, context: PolicyContext)
        -> Result<PolicyDecision, PolicyError>;
}

trait ApprovalReadPort {
    async fn verified_resolution(&self, approval_id: ApprovalId)
        -> Result<ApprovalResolution, ApprovalError>;
}

trait ExecutionTargetPort {
    async fn execute(&self, request: VerifiedExecution)
        -> Result<TypedExecutionResult, TargetError>;
    async fn reconcile(&self, execution_id: ExecutionId)
        -> Result<TargetExecutionStatus, TargetError>;
}
```

The reusable logic implements synchronous `PolicyPort::evaluate`, `PermitStore::issue`, and
`PermitStore::consume`, plus `CapabilityBroker::authorize` and `CapabilityBroker::claim`. These are
generic contract and logic foundations. Universal `execute`, `reconcile`, multi-target ports, and
the production target loop remain future boundaries; no checkpoint driver is enabled by this PR.
The Broker has no Task storage dependency.

Neutral IDs and wire DTOs are implemented in the side-effect-free
`cosh-gateway-contracts` leaf. Policy adapters can
reuse `cosh-types` audit types without moving Task/Gateway contracts into `cosh-types`.

## Capability request schema

The implemented leaf request contains request, Task, Run and Actor identity, `TargetRef`, a
namespace/name/arguments-digest operation descriptor, a separate complete canonical operation
digest, requested resource/access scope, input digest, and expiry. Trusted ingress code must
canonicalize and hash the complete operation before constructing the request and the independent
`AuthoritativeRequestBinding`. `RequestContext` supplies the current time, parent binding, and
authoritative target/descriptor/digest/scope. The Broker compares every field before policy and
does not rebuild authority from presentation fields.
The extended target schema below remains the architecture target; runtime principal, lease fence,
effect classification, typed operation variants, and prior approval correlation are not yet
represented by the first slice.

```text
CapabilityRequest {
  request_id, task_id, run_id, tool_use_id?,
  actor_context, runtime_principal,
  target_ref, expected_target_kind,
  operation: CapabilityOperation,
  resource_scope, effect_class,
  canonical_input_digest,
  run_lease_fence, issued_at, deadline,
  prior_approval_id?
}
```

`CapabilityOperation` is a closed, versioned enum for supported operations:

```text
FileRead, FileWrite, DirectoryList, ProcessInspect, ProcessSignal,
PackageQuery, PackageInstall, PackageRemove,
ServiceQuery, ServiceStart, ServiceStop, ServiceRestart,
CheckpointList, CheckpointCreate, CheckpointRestore,
NetworkConnect, ShellCommand, PtyAttach, SkillInvoke, McpToolInvoke
```

Each variant carries typed fields and explicit limits. Unknown operations fail with
`unsupported_capability`; they never fall back to `ShellCommand`. `raw` strings may be retained as
bounded audit display data but are excluded from policy matching unless a specific parser has
normalized them.

Effect classes are `Observe`, `WorkspaceWrite`, `HostMutation`, `PrivilegedMutation`,
`ExternalNetwork`, and `InteractiveControl`. Classification is an input floor: a policy may raise
risk but cannot lower a typed operation below its built-in minimum.

## Target identity

`TargetRef` is a user-facing selection and never appears in a permit. Before policy evaluation,
`TargetResolver` pins it to an immutable `TargetIdentity`:

```text
TargetIdentity {
  target_kind,
  installation_id,
  machine_or_instance_identity,
  boot_or_agent_epoch,
  execution_namespace,
  workspace_root_identity?,
  effective_uid,
  platform_fingerprint
}
```

For a local target, identity is derived from daemon installation, pinned workspace/namespace,
machine and boot identity, and effective credentials. For a remote GuestOS target, Phase 0 must
define authenticated instance/agent epoch and replay resistance. A hostname, IP, display label,
workspace path string, channel installation, or caller-provided instance ID is insufficient.

Target changes after authorization invalidate the decision. Symlink, mount namespace, container,
UID, boot, agent epoch, and workspace-root changes are part of target revalidation where relevant.

The generic in-memory permit binds exact `TargetRef` but does not provide universal immutable
identity or attestation. No production target resolver is enabled in this task-only profile.
Canonical workspace identity, ws-ckpt endpoint pinning, remote identity, and multi-target attestation
remain future optional capability work.

## Decision and approval flow

The reusable logic implements deny, approval request, and permit. Durable asynchronous resolution,
approval-bound re-authorization, and target execution remain future work. The full flow below is the
design requirement for every future enabled side-effecting operation.

The Broker returns one of:

```text
Denied { reason_code, policy_revision }
ApprovalRequired { approval_spec, operation_digest, target_digest, expires_at }
Permitted { permit }
AlreadyExecuting { execution_id, status }
ReconciliationRequired { execution_id, reason_code }
```

Flow:

1. Validate schema, actor/runtime principal, Task/Run binding, lease fence, deadline, and limits.
2. Resolve and pin `TargetIdentity`; canonicalize operation and resource scope.
3. Compute operation and target digests, then evaluate built-in risk floor and loaded policy.
4. Persist and audit denial, or return `ApprovalRequired` to `TaskCoordinator`.
5. The coordinator commits `ApprovalRequested`; presentation delivers it asynchronously.
6. The coordinator commits the first valid resolution and re-submits the same capability request
   with `ApprovalId` and approval revision.
7. The Broker reads and verifies that resolution, re-resolves target and policy, then issues a
   permit that is no broader and no longer-lived than the approved specification.
8. Execution consumes the permit through the ledger and invokes the target adapter.

Policy or target changes between approval and permit issuance force re-evaluation. A more
restrictive result denies or requests new approval; an approval is never carried across a widened
scope.

## Permit contract

The implemented `ExecutionPermit` binds permit/request/execution IDs, actor, Task, Run, exact
target, complete operation digest, policy revision, optional approval ID, expiry, and
`single_use = true`.
It does not yet carry immutable target identity, runtime/lease fence, durable issuance timestamps,
revocation state, or cross-process integrity proof.

```text
CapabilityPermit {
  permit_schema_version,
  permit_id, execution_id,
  task_id, run_id, actor_id, runtime_principal,
  target_identity_digest,
  operation_kind, operation_digest, resource_scope_digest,
  policy_revision, approval_id?, approval_revision?,
  run_lease_fence,
  issued_at, not_before, expires_at,
  use_limit = 1,
  broker_nonce, integrity_proof
}
```

Phase 1 local execution SHOULD use an opaque ledger-backed permit handle plus integrity proof,
rather than a self-contained broad bearer token. `PermitVerifier` validates every field, current
target identity, expiry, fence, and ledger state. Permit serialization is bounded and excludes
raw command, secret, output, or credential values.

Invariants:

- one permit maps to one `ExecutionId`, one exact operation digest, and one target digest;
- a permit cannot be transferred across actor, Task, Run, Runtime, target, workspace, or boot;
- a used, expired, revoked, stale-fence, malformed, or unknown permit fails closed;
- permit renewal is not supported; a fresh request and current policy produce a new permit;
- approval may narrow the requested operation but cannot issue a wildcard permit;
- target adapters never accept an unpermitted typed operation or raw shell fallback.

## Transaction, idempotency, and execution ledger

Authorization deduplicates by `(TaskId, RunId, RequestId, operation_digest, target_digest)`. A retry
returns the original denial, approval specification, or still-valid unconsumed permit. Reusing a
`RequestId` for another digest returns `idempotency_conflict`.

The memory store atomically records permit metadata for logic tests only. The generic ledger schema
reserves permit metadata, `ExecutionId`, policy/approval references, expiry, and execution state;
no production target consumes these records in this PR. Security-boundary audit persistence is a
precondition for any future production authority.

The current `claim` validates actor, Task, Run, Execution, target, operation digest, policy
revision, expiry, and single-use state under the same mutex, then consumes the permit. The target
execution design atomically transitions `Ready -> Claimed` using permit ID, fence, and a target
executor
claim. Before the effect, it records `Started` audit evidence. The target returns a typed result
and reconciliation evidence; the ledger transitions to `Succeeded`, `Failed`, or `Uncertain`.
Repeated execute calls return the stored terminal result or `execution_in_progress`; they never
create another effect.

A crash after `Claimed` or `Started` can leave the effect unknown. Recovery asks
`ExecutionTargetPort.reconcile(ExecutionId)`. It does not reset the permit to `Ready`. If the
target cannot prove a terminal result, status becomes `Uncertain`, the Task suspends, and an
operator-safe reconciliation decision is required.

## Shell and typed operator rules

Typed `cosh-platform` operations are preferred because their action and resource fields can be
bound exactly. Existing `cosh-cli` remains a user-facing envelope; the Broker SHOULD call typed
platform adapters or a narrowly defined operator protocol, not parse arbitrary CLI output to infer
authority.

`ShellCommand` is an exceptional operation:

- tokenize before classification, including tab/newline separators;
- reject shell metacharacters and compound/unspaced variants unless an isolated, explicit
  high-risk executor contract supports them;
- bind exact argv, executable identity, cwd/workspace identity, selected environment names,
  UID, timeout, output budget, and target;
- never allow a permit for a prefix, free-form continuation, or inherited interactive shell;
- require a separate `PtyAttach` permit for interactive ownership;
- fail closed when parsing, executable resolution, target pinning, or policy classification is
  incomplete.

In the enabled brokered cosh-core profile, the only tool is `ask_user_question`, which has no OS
side effect. Direct Shell, Skill, MCP, extension, hook, file, process, network, and checkpoint
execution is disabled; there is no generic allow response or Shell fallback. Shell attachment and
owner migration are Phase 2.

## Security audit and Task correlation

Task events and security audit events remain separate. The Broker emits audit events for request,
policy result, approval correlation, permit issuance/denial/revocation, execution start, terminal
result, and uncertainty. Events carry bounded `TaskId`, `RunId`, `RequestId`, `ToolUseId`,
`ExecutionId`, policy revision, target digest, result code, duration, and redaction status.

Sensitive values are represented by digests or opaque evidence references. The existing audit
store's security-boundary durability behavior is required for permit issuance and execution
start. Best-effort audit mode cannot authorize privileged mutation in the Broker path.

## Error model

Stable categories include `invalid_capability`, `unsupported_capability`, `forbidden`,
`approval_required`, `approval_invalid`, `approval_expired`, `target_unresolved`,
`target_changed`, `policy_changed`, `idempotency_conflict`, `permit_expired`, `permit_revoked`,
`permit_consumed`, `permit_scope_mismatch`, `stale_lease`, `audit_unavailable`,
`execution_in_progress`, `execution_uncertain`, `target_unavailable`, and `internal`.

Errors distinguish safe same-request retry, new authorization, new approval, target
reconciliation, and non-retryable denial. They never echo secret inputs or unbounded target
output. Transport timeout is not evidence that an effect did not occur.

## Migration and compatibility

1. Freeze Phase 0 capability, target identity, permit, audit correlation, and approval schemas.
2. Introduce the policy boundary and in-memory permit ledger. **Pure logic implemented.**
3. Add persistent permit/execution ledger and required audit boundary. **Generic foundation only;
   production execution remains unimplemented.**
4. Admit only `core` / `gateway-brokered-v1` with the task-only inventory in production `serve`; keep old CLI and ACP
   `doctor`/`run` explicitly ungoverned.
5. Use private COSH brokered v3, bind the pinned `task-only-v1` manifest, and expose only
   `ask_user_question`.
6. Add the optional `workspace-checkpoint-v1` profile, its sealed provider set, and the checkpoint
   transport's effect classification. **Provider admission withheld; no execution target.**
7. Land the ws-ckpt protocol prerequisites, then implement the checkpoint execution target, wire the
   Runtime checkpoint request through durable approval and a single-use permit, and mirror the second
   profile on the private Core wire. **Not implemented.**
8. Keep Shell/ACP/Skills/MCP/extensions disabled until a later phase provides complete adapters.
9. Remove or explicitly isolate legacy bypasses only after parity and recovery acceptance passes.

The optional profile never changes the portable one. The `task-only-v1` canonical manifest stays
byte-identical to its original revision, so the private Core v3 mirror keeps verifying the same pinned
digest, and a Task-only instance keeps starting with no ws-ckpt package, socket, service, or provider.
An unavailable ws-ckpt daemon can only make a checkpoint-enabled instance refuse admission; it can
never block a Task-only instance.

Rollback preserves standalone Shell and legacy binaries. Production `serve` remains fail-closed;
it cannot fall back to ACP or a legacy mutation backend. A release may advertise only the
task-only `ask_user_question` profile, never “all side effects governed.”

## Dependencies

- Phase 0 identity, target, capability, schema compatibility, storage, secret, and threat-model
  decisions.
- [Task Execution Plane](../task-execution-plane/design.md): Task/Run state, durable approval, and
  result recording.
- [Gateway API](../gateway-api/design.md): actor and direct-operation ingress.
- [Cosh Core Bridge](../cosh-core-bridge/design.md): JSONL tool-intent translation and brokered
  runtime profile.
- `cosh-platform` typed operations and audit policy/storage remain implementation foundations.

## Implementation work breakdown

1. Define capability, target, approval reference, permit, and execution result schemas.
2. Implement target resolution/pinning and canonical operation/resource digests.
3. Adapt current audit policy evaluation with built-in minimum effect classification.
   **Policy wiring for future side-effecting capabilities remains open.**
4. Implement decision flow and durable approval correlation without Task writes.
   **Branching is generic; production side-effect resolution remains open.**
5. Implement permit issuance, verification, revocation, consume, and execution ledger.
   **Generic contract foundations exist; no production ExecutionTarget is wired.**
6. Keep checkpoint/ws-ckpt and strict Shell/Pty adapters disabled until a future optional
   capability profile supplies complete policy, audit, and recovery evidence.
7. Add required pre-effect audit and Task correlation before any future target execution.
8. Integrate Gateway and brokered COSH v2; ACP hosting and presentation expansion remain
   later-phase work.
9. Freeze and test each future capability inventory without claiming universal bypass coverage.

## Test strategy

Eight current unit tests cover request expiry and parent substitution, deny and approval branches,
policy failure and invalid authority, complete permit binding, binding mismatch without
consumption, expiry/replay, and eight-way concurrent claim. The broader security suite remains:

- Schema golden/property tests for stable digests and ID type separation.
- Table tests for built-in risk floor plus every policy decision and approval transition.
- Adversarial Shell corpus covering tabs, newlines, unspaced metacharacters, path substitution,
  symlink/mount changes, environment injection, and executable replacement.
- Target substitution tests across workspace, UID, boot/agent epoch, container, and remote instance.
- Permit tests for expiry, replay, tampering, stale fence, cross-actor/Task/target use, and revoke.
- Concurrent consume tests proving one permit produces at most one claimed Execution ID.
- Kill-point tests before/after claim, audit start, OS invocation, result capture, and Task callback.
- Reconciliation tests for typed success, typed failure, in-progress, and unknown effects.
- Bypass tests proving the packaged production service stays task-only with no ws-ckpt dependency,
  plus future tests for each Gateway/Core/Shell/ACP/Skill/MCP mutation path before it may be enabled.

The optional profile and its withheld provider add deterministic coverage, with no real ws-ckpt daemon,
btrfs filesystem, ECS instance, or manual terminal involved:

- Profile tests pinning the second canonical manifest and digest, and rejecting a missing, extra,
  reordered, or renamed tool and any provider-set drift, including the previously rejected
  `ws-ckpt-v1` name.
- Registry tests proving a Task-only instance admits an empty provider set with no ws-ckpt configuration
  present and rejects a requested provider, that a checkpoint-enabled instance refuses admission without
  one, and that the checkpoint provider is withheld for the profile that seals it.
- An exhaustive test over every profile and requested-provider combination proving no admission outcome
  yields checkpoint side-effect authority.
- Transport tests classifying every request phase as proven no-effect or possibly applied, including all
  thirteen daemon error codes, and covering peer authentication and the exact read-only reconcile query.

## Open questions

| Question | Owner | Phase 1 default |
| --- | --- | --- |
| What is the canonical local/remote target identity? | Phase 0 identity/security | Local pinned identity only; remote blocked. |
| Is the permit opaque or signed across processes? | Broker/security | Opaque ledger-backed local handle; integrity proof at process boundary. |
| Which audit mode gates mutation? | Security/audit | Required for permit issuance and execution start. |
| Can opaque compound shell ever be permitted? | Security/executor | Deny in initial profile; prefer typed operator. |
| How is a post-crash effect reconciled? | Target owner | Per-operation typed probe; unknown suspends Task. |
| When is legacy direct CLI removed? | Product/release | After parity, recovery, and bypass inventory acceptance. |
