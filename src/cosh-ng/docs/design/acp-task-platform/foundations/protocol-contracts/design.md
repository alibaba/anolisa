# Phase 0 Protocol Contracts Design

[中文版](design_zh.md) | [Acceptance report](acceptance.md) |
[Planning set](../../README.md)

## Status and decision

- Status: Gateway/Task schema v1 and Runtime contract schema v4 are implemented;
  production inventory is task-only and the aggregate Gate remains open
- ACP profile: wire protocol v1 with `initialize.protocolVersion = 1`

The side-effect-free
[`cosh-gateway-contracts`](../../../../../crates/cosh-gateway-contracts/src/lib.rs)
crate now implements the first COSH-owned domain-contract slice. ACP types
remain inside the ACP bridge. Existing cosh-core JSONL messages remain inside
the CoshCore bridge. The Task Execution Plane speaks only the neutral commands,
events, runtime messages, and capability requests defined here. Gateway/Task
schema remains v1, Runtime contract independently advances to v4, and storage
schema is v9. The local ACP bridge, durable Task/Outbox foundations, Runtime
port, production daemon factory, local actor, inode-bound workspace admission,
and asynchronous approval are implemented. The production profile is task-only
with `ask_user_question`; no production `ExecutionTarget` or checkpoint/ws-ckpt
path is wired. Generic capability governance remains a future optional
capability. The complete compatibility manifest, remote identity, and universal
execution coverage remain later Gates.

The ACP protocol version and SDK package version are independent. The candidate
bridge negotiates ACP protocol `1`, pins official SDK 2.0.0, and raises the
cosh-ng Rust/RPM baseline to 1.88. An SDK package major is never inferred from
the wire version.

## Goals

- Freeze versioned Task, Runtime, Capability, Approval, and presentation event
  envelopes shared by Phases 1 and 2.
- Prevent ACP, cosh-core, Shell, HTTP, DingTalk, or Feishu payloads from
  becoming durable domain objects.
- Define command idempotency, event ordering, cancellation, terminal outcome,
  and error semantics before storage code exists.
- Preserve the current standalone `cosh-shell` crate boundary.
- Provide schemas and golden fixtures that bridges can test independently.

## Non-goals

- Defining the production Gateway API, remote scheduler topology, or channel
  authentication contract.
- Replacing the existing shell/core control protocol in Phase 0.
- Standardizing a remote ACP transport. Phase 2 uses local stdio only.
- Defining provider prompts, model APIs, OS policy rules, or channel-specific
  authentication payloads.
- Promising exactly-once external side effects. The contract provides durable
  admission and idempotent execution identities; executors must still prove
  their own replay behavior.

## Current-source evidence

| Evidence | Baseline fact | Contract implication |
| --- | --- | --- |
| [`CONTROL_PROTOCOL_VERSION`](../../../../../crates/cosh-core/src/protocol.rs#L9) and [`InputMessage`](../../../../../crates/cosh-core/src/protocol.rs#L60) | cosh-core accepts a product-specific JSONL protocol with exact version `1` | This is not ACP v1 and must remain behind `CoshCoreBridge` |
| [`OutputMessage`](../../../../../crates/cosh-core/src/protocol.rs#L202) and [`CoreControlRequest`](../../../../../crates/cosh-core/src/protocol.rs#L360) | Streaming, approval, questions, and shell evidence use core-specific shapes | Bridge translation is required; these types cannot enter Task storage |
| [`AgentAdapter`](../../../../../crates/cosh-shell/src/adapter/mod.rs#L87) and [`AgentEvent`](../../../../../crates/cosh-shell/src/types/mod.rs#L402) | A useful lifecycle boundary exists, but IDs and payloads are Shell-owned strings | Reuse semantics, not Rust types, in the neutral Runtime Port |
| [`PersistedSession`](../../../../../crates/cosh-core/src/session.rs#L83) | Provider conversation history already has a versioned envelope | Provider Session remains separate from durable Task and Event contracts |
| [`AuditIdentity`](../../../../../crates/cosh-shell/src/types/audit.rs#L29) | Cross-runtime correlation fields exist for audit events | The Phase 0 identity module extends and types this vocabulary |
| [Runtime contracts](../../../../../docs/design/runtime-contracts.md) | Current ownership and negotiation are documented as implemented | Migration must keep this path usable until the bridge is accepted |

The implementation includes the neutral contracts, ACP bridge, durable Task
and Outbox foundations, and a behavioral Runtime Port. The daemon executable
schedules the contained task-only Core profile under fenced leases, persists
`RuntimeBound` before prompting, and resolves provider-native approvals
durably. Provider-native tool execution remains observed rather than enforced
by COSH. The production profile exposes only `ask_user_question`; no
production `ExecutionTarget` or checkpoint/ws-ckpt path is wired. Generic
Capability/Permit/Execution contracts and ledger rows remain future optional
capability foundations and do not constitute a passed execution loop.

## Module ownership

| Owner | Planned responsibility | Explicit exclusions |
| --- | --- | --- |
| `cosh-gateway-contracts` leaf crate | Pure serializable IDs, command/event envelopes, error codes, and enums; schema fixtures remain pending | I/O, async traits, ACP SDK types, database records |
| Future `cosh-gateway::ports` | Behavioral traits for `TaskStore`, `AgentRuntimePort`, `CapabilityBrokerPort`, and `PresentationPort` | Transport parsing and provider-specific logic |
| `cosh-gateway` coordinator | Validate commands, enforce state transitions, append events, and invoke ports | Parsing ACP or cosh-core JSONL directly |
| `CoshCoreBridge` | Translate neutral Runtime messages to the existing JSONL protocol | Owning Task state or authorization decisions |
| `AcpClientBridge` | Translate ACP v1 SDK types and callbacks to neutral Runtime and Capability contracts | Serving channel traffic or persisting ACP wire messages |
| `cosh-shell` attachment | Mirror the public Gateway wire contract and verify it with shared fixtures | Depending on internal crates or supervising Agent processes after migration |

`cosh-gateway-contracts` is implemented as a side-effect-free leaf crate, not a
module of the existing `cosh-types`. G0 still requires an accepted ownership
ADR and versioned schemas/fixtures. Ownership and dependency direction are
frozen: pure types point inward, transports and bridges point toward them, and
no domain crate depends on an SDK. This choice
does not change the standalone/no-internal-dependency rule for `cosh-shell`;
the first Shell attachment uses a Gateway wire client/mirror plus canonical
fixtures. A direct dependency on the leaf crate requires a separate ADR.

## Contract layers

```text
Channel / Shell / Web payload
          |
          v
Gateway command envelope ---------> Task command
                                         |
                                         v
                                  durable Task event
                                         |
                  +----------------------+-------------------+
                  v                                          v
       AgentRuntimePort command                    Presentation event
                  |
                  v
       Capability request -> decision -> execution result
```

Only Task events are authoritative lifecycle facts. Runtime updates and
presentation events become durable only after the coordinator validates and
records them.

## Typed contract

The committed modules are
[`task`](../../../../../crates/cosh-gateway-contracts/src/task.rs),
[`runtime`](../../../../../crates/cosh-gateway-contracts/src/runtime.rs),
[`capability`](../../../../../crates/cosh-gateway-contracts/src/capability.rs),
and [`error`](../../../../../crates/cosh-gateway-contracts/src/error.rs). The
following abbreviated definitions describe their stable semantics; committed
Rust types are authoritative for field-level detail. All identifiers are
validated newtypes from the identity module.

```rust
struct ContractHeader {
    schema: &'static str,
    schema_version: u16,
    message_id: MessageId,
    occurred_at_ms: u64,
    correlation: Correlation,
}

struct GatewayCommandEnvelope {
    header: ContractHeader,
    actor: ActorRef,
    idempotency_key: IdempotencyKey,
    expected_task_revision: Option<u64>,
    command: TaskCommand,
}

enum TaskCommand {
    CreateTask { intent: BoundedText, target: TargetRef },
    StartRun { task_id: TaskId, runtime: RuntimeSelector },
    SubmitInput { task_id: TaskId, content: Vec<ContentPart> },
    ResolveApproval { approval_id: ApprovalId, decision: ApprovalDecision },
    CancelRun { task_id: TaskId, run_id: RunId, reason: CancelReason },
    Attach { task_id: TaskId, cursor: Option<EventCursor> },
}

struct TaskEventEnvelope {
    header: ContractHeader,
    task_id: TaskId,
    revision: u64,
    event: TaskEvent,
}

enum TaskEvent {
    TaskSubmitted { intent_digest: Digest, target: TargetRef },
    TaskQueued { run_id: RunId, runtime: RuntimeSelector },
    RunStarted { run_id: RunId },
    RuntimeBound { run_id: RunId, binding: RuntimeBindingRef },
    RuntimeEventRecorded { run_id: RunId, update: RuntimeUpdate },
    InputRequested { request: RuntimeInputRequest },
    InputSubmitted { request_id: InputRequestId, run_id: RunId, response_digest: Digest },
    ApprovalRequested { approval: ApprovalRequest },
    ApprovalResolved { approval_id: ApprovalId, decision: ApprovalDecision },
    ExecutionPlanned { execution_id: ExecutionId, permit_id: PermitId },
    ExecutionResultRecorded { execution_id: ExecutionId, outcome: ExecutionOutcome },
    ExecutionUncertain { execution_id: ExecutionId, reason: UncertaintyCode },
    CancellationRequested { run_id: RunId, cause: CancelReason },
    RunCancelled { run_id: RunId, stage: CancellationStage },
    RunSuspended { run_id: RunId, reason: SuspensionCode },
    RunSucceeded { run_id: RunId },
    RunFailed { run_id: RunId, error: ContractError },
    RunRetryQueued { previous_run_id: RunId, next_run_id: RunId },
    TaskSucceeded,
    TaskFailed { error: ContractError },
    TaskCancelled,
}

enum AgentRuntimeCommand {
    OpenSession { task_id: TaskId, run_id: RunId, workspace: WorkspaceRef },
    ResumeSession { task_id: TaskId, run_id: RunId, binding: RuntimeBindingRef },
    Prompt { run_id: RunId, turn_id: TurnId, input: Vec<ContentPart> },
    ResolvePermission { request_id: RequestId, decision: RuntimePermissionDecision },
    ResolveInput { request_id: InputRequestId, run_id: RunId, turn_id: TurnId,
                   response: RuntimeInputResponse },
    Cancel { run_id: RunId, turn_id: TurnId, cause: CancelReason },
    Close { binding: RuntimeBindingRef },
}

enum AgentRuntimeEvent {
    SessionOpened { binding: RuntimeBindingRef },
    TurnStarted { turn_id: TurnId },
    MessageChunk { message_id: RuntimeMessageId, content: ContentPart },
    ToolCallObserved { tool_use_id: ToolUseId, summary: ToolSummary },
    ToolInvocationUpdated { snapshot: ToolInvocationSnapshot },
    PermissionRequested { request: CapabilityRequest },
    ExecutionPermissionRequested {
        turn_id: TurnId,
        tool_use_id: Option<ToolUseId>,
        summary: ToolSummary,
        request: CapabilityRequest,
        authority: ExecutionAuthority,
    },
    InputRequested { request: RuntimeInputRequest },
    UsageUpdated { usage: RuntimeUsage },
    Completed { turn_id: TurnId, outcome: TurnOutcome },
    TransportFailed { error: RuntimeError },
}
```

Required wire schemas are named `cosh.gateway.command`, `cosh.task.event`,
`cosh.runtime.command`, and `cosh.runtime.event`. Gateway commands and Task
events remain at schema version `1`. Runtime commands and events use schema
version `4`; the current revision includes explicit Turn outcomes, brokered
delivery, and typed input request/response exchange. Domain schema versions are unrelated to ACP
`protocolVersion`; the ACP bridge still negotiates wire protocol `1`.

`TurnOutcome` terminates one prompt turn and never settles the owning Run or
Task by itself. Limits, refusal, cancellation, and failure remain distinct so
the coordinator can apply an explicit Task policy. A Session may accept a new
Turn after normal completion.

Runtime input uses a bounded typed request and response. `InputRequested`
persists the complete presentation and pending identity, while
`InputSubmitted` persists only the response digest. The raw response remains in
the private dispatch ledger and is never copied into Task history or command
receipts.

`ExecutionAuthority` records the actual enforcement boundary. ACP
provider-native tool execution is `ProviderNativeObserved`: COSH may present
and audit the provider's permission exchange but cannot claim that its permit
was consumed at the side-effect boundary. `CoshBrokered` is reserved for an
operation executed through a COSH `ExecutionTarget`, where the bound permit is
atomically consumed before the side effect.

The `summary` on an execution-permission event is bounded, sanitized
presentation only. Agent-provided presentation never selects an authority,
operation digest, target, or provider response.

### Capability contract

```rust
struct CapabilityRequest {
    request_id: RequestId,
    task_id: TaskId,
    run_id: RunId,
    actor: ActorRef,
    target: TargetRef,
    operation: OperationDescriptor,
    operation_digest: Digest,
    requested_scope: CapabilityScope,
    input_digest: Digest,
    expires_at_ms: u64,
}

enum CapabilityDecision {
    Permit { permit: ExecutionPermit },
    RequireApproval { approval: ApprovalRequest },
    Deny { code: DenialCode, safe_message: BoundedText },
}

struct ExecutionPermit {
    permit_id: PermitId,
    request_id: RequestId,
    target: TargetRef,
    operation_digest: Digest,
    valid_until_ms: u64,
    single_use: bool,
}
```

A permit is bound to target, normalized operation digest, requesting Run, and
expiry. Bridges never manufacture permits. ACP `session/request_permission`
is translated to this request and only a broker decision is translated back.

`operation_digest` covers the complete canonical namespace, operation name,
and normalized arguments. `OperationDescriptor.arguments_digest` is narrower
policy detail and cannot be used as permit authority. Trusted admission owns
canonicalization and hashing, then pins target, the complete descriptor,
operation digest, and requested scope before Broker policy evaluation.

### Error envelope

```rust
struct ContractError {
    code: ErrorCode,
    category: ErrorCategory,
    retryable: bool,
    safe_message: BoundedText,
    retry_after_ms: Option<u64>,
    details_ref: Option<EvidenceRef>,
}

enum ErrorCategory {
    InvalidRequest,
    Conflict,
    NotFound,
    Unauthorized,
    PolicyDenied,
    RuntimeUnavailable,
    Transport,
    Storage,
    Cancelled,
    Internal,
}
```

Raw provider errors, stderr, secrets, prompts, and stack traces never enter
`safe_message`. Detailed diagnostics use bounded, redacted evidence.

## State and sequence semantics

### Command admission

1. Resolve and authenticate `ActorRef` before constructing the domain command.
2. Look up `(actor, idempotency_key, command_kind)`.
3. If an identical payload digest was accepted, return its original result.
4. If the key exists with another digest, return `idempotency_conflict`.
5. Validate `expected_task_revision` when supplied.
6. Append Task event, update projection, and enqueue Outbox entries in one
   storage transaction.
7. Invoke Runtime or presentation work only after the transaction commits.

### Runtime turn

```text
TaskSubmitted -> TaskQueued -> RunStarted -> RuntimeEventRecorded*
                                  |                  |
                                  |                  +-> ApprovalRequested
                                  |                          -> ApprovalResolved
                                  |                          -> ExecutionPlanned
                                  |                          -> ExecutionResultRecorded
                                  +-> RunSucceeded | RunCancelled | RunFailed | RunSuspended
```

- Exactly one terminal Run event is accepted for a Run revision.
- Late Runtime updates are retained as diagnostics, not applied to a terminal
  projection.
- Runtime session IDs and JSON-RPC request IDs are scoped opaque references;
  neither determines Task identity.
- A cancellation request is durable before transport cancellation begins.
  Completion that wins the race remains completion; otherwise the coordinator
  records the final cancellation stage.
- Event cursors address the durable Task sequence, not an ACP notification
  order or a process-local channel offset.

## ACP v1 compatibility profile

Phase 2 must implement the stable v1 baseline methods `initialize`,
`session/new`, `session/prompt`, `session/cancel`, and `session/update`.
Optional lifecycle, content, filesystem, terminal, elicitation, and config
features are enabled only when negotiated. The bridge must:

- send `protocolVersion: 1` and close cleanly if the Agent selects an
  unsupported version;
- treat omitted capabilities as unsupported;
- use local newline-delimited JSON-RPC over stdin/stdout;
- preserve ACP Session IDs, JSON-RPC IDs, message IDs, and tool call IDs as
  opaque external references;
- translate `session/request_permission`, `terminal/*`, and `fs/*` into COSH
  governance rather than executing them in the protocol reader;
- implement `session/cancel` semantics and capability-gated
  `$/cancel_request` without equating transport cancellation with durable Task
  completion;
- keep the draft ACP Streamable HTTP transport outside the Phase 0-2 contract.

Normative references: [ACP v1 initialization](https://agentclientprotocol.com/protocol/v1/initialization),
[prompt turn](https://agentclientprotocol.com/protocol/v1/prompt-turn),
[cancellation](https://agentclientprotocol.com/protocol/v1/cancellation), and
[transports](https://agentclientprotocol.com/protocol/v1/transports).

## Error and security boundaries

- Unknown schema versions fail before state mutation.
- Unknown enum values are rejected unless the field is explicitly declared
  forward-compatible and retained as bounded opaque metadata.
- Deserializers enforce byte, collection, nesting, and text limits.
- External paths are canonicalized at the Capability Broker; a path in an ACP
  message is not an authorization grant.
- Channel actors cannot supply internal IDs, ownership, policy results, or
  event revisions.
- Runtime updates are untrusted input until correlated with the active fenced
  Runtime binding.
- Approval responses require actor authorization and the exact pending
  `ApprovalId`; a JSON-RPC response alone does not authorize execution.
- Secrets and full environment maps are forbidden in durable contracts.

## Compatibility and migration

1. Add types, schemas, and fixtures without changing existing JSONL behavior.
2. Wrap the current `AgentAdapter` and cosh-core JSONL path in a
   `CoshCoreBridge` that emits neutral Runtime events.
3. Run bridge conformance fixtures alongside current protocol tests.
4. Move durable ownership to the Gateway only after Phase 1 acceptance.
5. Migrate Shell to attachment mode in Phase 2; retain an explicit fallback
   to the current local runtime during the compatibility window.
6. Remove or change a v1 field only through a schema-v2 decision and migration
   fixtures. Additive optional fields remain v1-compatible.

Existing `ProviderSessionId` files, audit segments, and shell/core control
protocol version `1` are not rewritten by this plan.

## Dependencies

- [Identity and correlation](../identity-correlation/design.md) freezes ID
  constructors, scopes, and inheritance.
- [Storage and supervision](../storage-supervision/design.md) freezes atomic
  persistence and child-process ownership.
- Phase 1 Gateway, Task Plane, Broker, and CoshCore Bridge consume these
  contracts.
- Phase 2 ACP, Shell, and Web modules provide transport adapters only.

## Implementation tasks

1. Add pure newtypes and envelopes to the selected contract owner. **Done.**
2. Publish JSON Schema and canonical JSON fixtures for every message kind.
3. Add bounded deserialization and stable error codes. **Scalar values and
   errors are bounded; aggregate collection totals remain open.**
4. Define port traits using only neutral types.
5. Add state-machine reducers with property tests for terminal uniqueness and
   revision monotonicity. **Critical reducer tests exist; the property matrix
   remains open.**
6. Add CoshCore translation fixtures against the existing JSONL protocol.
7. Add ACP v1 fixtures generated by the pinned official SDK.
8. Add a compatibility manifest mapping product schema, ACP protocol, SDK,
   and cosh-core control versions independently.

## Test strategy

- Schema tests validate every golden example and reject unknown required
  fields, oversized inputs, and invalid ID scopes.
- Round-trip tests cover JSON serialization without relying on field order.
- State-machine property tests permute duplicate, delayed, cancelled, and
  terminal events.
- Bridge contract tests run without a real model or host mutation.
- ACP conformance uses a deterministic fake Agent over stdio.
- Security tests attempt cross-task ID substitution, permit replay, stale
  Runtime generations, path escapes, and secret-bearing errors.

## Open decisions

| Decision | Owner | Must close by |
| --- | --- | --- |
| Accept the implemented `cosh-gateway-contracts` ownership in an ADR and land its v1 schemas/fixtures | cosh-ng maintainers | Before G0 exit |
| UUIDv4-to-UUIDv7 allocator migration and compatibility | Identity module | Identity contract review |
| Public Gateway wire encoding beyond JSON v1 | Gateway API module | Phase 1 API freeze |
| Which ACP stabilized optional capabilities enter the first conformance profile | ACP bridge owner | Phase 2 implementation start |
| Duration of the Shell fallback compatibility window | Product and runtime owners | Phase 2 rollout review |
