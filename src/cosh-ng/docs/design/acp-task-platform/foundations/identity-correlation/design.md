# Phase 0 Identity and Correlation Design

[中文版](design_zh.md) | [Acceptance report](acceptance.md) |
[Planning set](../../README.md)

## Status and decision

- Baseline: `up/main` at `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`
- Status: typed identity foundation implemented; G0 storage/admission exit remains open

COSH must assign its own typed lifecycle identities and preserve channel,
Shell, cosh-core, and ACP identifiers as scoped external references. Identity
equality is valid only for the same type and scope. In particular:

```text
TaskId != RunId != AgentSessionId != ShellSessionId
RequestId != ToolUseId != ExecutionId != ApprovalId
```

An external session or message identifier can locate a binding, but it can
never authorize an actor, select an OS target, or become a Task identifier.

## Goals

- Define canonical internal IDs, external references, ownership, and scope.
- Correlate one user intent across channel admission, Task events, Agent
  Runtime, approvals, OS execution, audit, and presentation delivery.
- Make duplicate delivery, stale Runtime output, and cross-task substitution
  detectable before side effects.
- Support actor resolution for local users, automation, and future DingTalk or
  Feishu adapters without storing channel credentials in domain events.
- Extend the existing audit vocabulary without silently changing its v1 wire
  contract.

## Non-goals

- Designing a full IAM, organization directory, OAuth flow, or channel login.
- Treating Linux UID alone as a globally stable human identity.
- Exposing internal identifiers as secrets. IDs are unguessable correlation
  handles, not authorization tokens.
- Reusing provider, ACP, JSON-RPC, or Shell IDs because their text happens to
  match.
- Migrating existing provider session filenames or audit events in Phase 0.

## Current-source evidence

| Evidence | Baseline fact | Gap |
| --- | --- | --- |
| [`ProviderSessionId`](../../../../../crates/cosh-core/src/session.rs#L23) | Persistence accepts only a canonical lowercase UUID and scopes it to a canonical workspace | It identifies provider history only, not a Task or actor |
| [`AuditIdentity`](../../../../../crates/cosh-shell/src/types/audit.rs#L29) | Audit events can carry installation, Shell session, provider session, Run, turn, request, tool, and command strings | Fields are optional strings and do not include Task, Approval, Execution, Delivery, actor, target, or Runtime generation |
| [`ShellEvent`](../../../../../crates/cosh-shell/src/types/mod.rs#L69) | Shell session and command identity are independent from provider audit identity | No durable Gateway binding exists |
| [`AgentRequest`](../../../../../crates/cosh-shell/src/types/mod.rs#L303) and [`AgentEvent`](../../../../../crates/cosh-shell/src/types/mod.rs#L402) | `AgentRequest.id` flows as a string Run ID through Shell events | Type and generation fencing are missing |
| [`ProviderToolKey`](../../../../../crates/cosh-shell/src/runtime/provider_tool_state.rs#L236) | In-memory tool correlation already requires `(run_id, tool_id)` | The scope disappears after Shell exit and is not a durable execution identity |
| [`RunCommand`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service/command.rs#L21) | Persistent core service carries Run ID and separate session scope | The process binding has no durable owner/generation record |

The implementation worktree adds the canonical
[`ids`](../../../../../crates/cosh-gateway-contracts/src/ids.rs),
[`common`](../../../../../crates/cosh-gateway-contracts/src/common.rs), and
[`external`](../../../../../crates/cosh-gateway-contracts/src/external.rs)
modules. A durable external-reference registry, actor resolver, and storage
constraints remain outside this leaf-contract slice.

## Ownership

| Owner | Responsibility |
| --- | --- |
| `cosh-gateway-contracts` leaf crate | ID newtypes, `Correlation`, `ExternalRef`, constructors' validation contract, and serializable scope types |
| Gateway `IdentityResolver` | Authenticate ingress, map issuer/subject to `ActorId`, and emit provenance |
| Task Coordinator | Allocate Task, Run, Approval, Execution, Delivery, and command Message IDs; enforce parent-child invariants |
| Runtime Supervisor | Allocate Runtime instance/generation and Connection IDs; register Agent Session bindings |
| Runtime bridges | Preserve provider and ACP values as opaque scoped references; never parse semantic meaning from them |
| Capability Broker | Bind actor, Task, Run, target, operation, permit, Approval, and Execution IDs |
| Audit projection | Add new optional correlation fields in a separately reviewed schema revision or map them through references |
| Channel adapters | Construct bounded issuer-specific external references and idempotency material; never assign internal ownership |

The G0 ownership ADR for `cosh-gateway-contracts` also governs these newtypes.
`cosh-shell` keeps its standalone boundary and mirrors Gateway wire IDs through
canonical fixtures unless a later ADR permits a direct dependency.

## Identity taxonomy

### COSH-assigned internal identities

| Type | Scope and lifetime | Parent/invariant |
| --- | --- | --- |
| `InstallationId` | One local Gateway installation; durable | Never derived from hostname or machine-id alone |
| `ActorId` | Principal known to this installation; durable | Maps from authenticated `(issuer, subject)` |
| `TaskId` | Durable user intent | Immutable owner and target policy context |
| `RunId` | One attempted Runtime turn or workflow run | Exactly one `TaskId`; retries get a new Run |
| `AgentSessionId` | COSH logical Agent conversation binding | One Task context; maps to provider/ACP external sessions without adopting their IDs |
| `RuntimeInstanceId` | One supervised child process | One launch specification and generation sequence |
| `RuntimeBindingId` | Binding between Run and external Agent Session | One Task, Run, Runtime instance, generation, and external session ref |
| `ApprovalId` | One durable decision request | One Task, Run, request digest, and policy revision |
| `PermitId` | One Broker authorization result | Bound to Approval if required, target, operation digest, expiry |
| `ExecutionId` | One attempted side effect | One permit; reused only for an executor's defined idempotent replay |
| `DeliveryId` | One Outbox delivery to one sink | One event and destination; attempts are separate counters |
| `MessageId` | One COSH command/event envelope | Globally unique within installation |

Internal IDs use a short type prefix plus canonical lowercase hyphenated UUIDv4
text, for example `tsk_<uuid>` and `run_<uuid>`. The allocator matches the
workspace's centralized `uuid` feature set and Rust 1.88 baseline. Durable
ordering uses Task revision or database sequence, never UUID ordering. A future
UUIDv7 allocator may retain the same prefixed text contract, but requires an
explicit compatibility decision.

### Scoped external identities

| Type | Required scope | Rule |
| --- | --- | --- |
| `ChannelConversationRef` | adapter + authority/tenant + conversation | Opaque and bounded; not an actor |
| `ChannelMessageRef` | conversation ref + message value | Supplies ingress deduplication material |
| `ShellSessionRef` | installation + Shell process/session | Never resumes provider context by itself |
| `ShellCommandRef` | Shell session + command ID | Identifies PTY evidence only |
| `ProviderSessionRef` | Runtime kind + workspace + provider session value | May map to current `ProviderSessionId`; never a Task ID |
| `AcpConnectionRef` | Runtime instance + generation | Allocated locally for stdio connection correlation |
| `AcpSessionRef` | ACP connection + opaque Agent session ID | May be reused across Runs only through an explicit binding |
| `AcpRequestRef` | ACP connection + JSON-RPC ID | JSON-RPC number and string forms remain distinct wire values |
| `AcpMessageRef` | ACP session + opaque optional message ID | Missing message ID requires local chunk sequence, not invented Agent identity |
| `AcpToolCallRef` | ACP session + opaque tool call ID | Maps to one internal tool observation, not directly to Execution |
| `TerminalRef` | Runtime binding + ACP terminal ID | Valid only while terminal ownership record exists |

External values are stored separately from their scope. No code concatenates
unescaped strings to invent a composite primary key.

## Typed schema

The committed source implements the following shapes. The abbreviated view
below omits helper methods and validation details.

```rust
struct Correlation {
    installation_id: InstallationId,
    actor_id: Option<ActorId>,
    task_id: Option<TaskId>,
    run_id: Option<RunId>,
    agent_session_id: Option<AgentSessionId>,
    runtime_binding_id: Option<RuntimeBindingId>,
    approval_id: Option<ApprovalId>,
    permit_id: Option<PermitId>,
    execution_id: Option<ExecutionId>,
    causation_message_id: Option<MessageId>,
}

struct ExternalRef {
    kind: ExternalRefKind,
    authority: BoundedName,
    scope_digest: Digest,
    value: BoundedOpaque,
}

struct ActorRef {
    actor_id: ActorId,
    actor_kind: ActorKind,
    issuer: BoundedName,
    assurance: AuthAssurance,
}

struct RuntimeBindingRef {
    binding_id: RuntimeBindingId,
    runtime_instance_id: RuntimeInstanceId,
    runtime_generation: u64,
    agent_session: ExternalRef,
}
```

`ExternalRef.value` may contain private tenant or user data. Domain and audit
events store an encrypted reference row ID or installation-keyed digest unless
the raw value is required for protocol continuation. Logs and errors use only
kind, digest, and safe suffix.

### Durable relation draft

```text
actors(actor_id, issuer, subject_digest, assurance, status)
tasks(task_id, owner_actor_id, target_ref, revision, ...)
runs(run_id, task_id, attempt, runtime_selector, ...)
agent_sessions(agent_session_id, task_id, runtime_kind, state, ...)
runtime_instances(runtime_instance_id, generation, launch_digest, ...)
runtime_bindings(binding_id, task_id, run_id, agent_session_id, runtime_instance_id,
                 runtime_generation, external_ref_id, status)
external_refs(external_ref_id, kind, authority, scope_digest,
              value_ciphertext_or_value, value_digest)
approvals(approval_id, task_id, run_id, request_digest, ...)
permits(permit_id, approval_id?, task_id, run_id, target_digest, ...)
executions(execution_id, permit_id, idempotency_scope, ...)
deliveries(delivery_id, event_id, sink_digest, attempt, ...)
```

Foreign keys and unique constraints enforce the parent relations. Event JSON
is not the only place where correlation exists.

## Correlation propagation

### Ingress to Task

1. Adapter verifies the transport credential and constructs issuer, subject,
   conversation, and message references.
2. `IdentityResolver` returns an `ActorRef`; failure stops before Task
   admission.
3. Gateway derives or accepts a bounded idempotency key. For channel messages,
   it is an installation-keyed digest of the complete scoped message ref.
4. Coordinator creates or replays a Task command and assigns `TaskId` and
   `MessageId`.
5. Raw credentials, webhook signatures, and bearer tokens are discarded
   outside the adapter boundary.

### Task to Runtime

1. Coordinator creates `RunId` under `TaskId`.
2. Supervisor selects or spawns a `RuntimeInstanceId` and increments its
   generation on every new process.
3. Coordinator selects or creates the COSH `AgentSessionId`; the bridge opens
   or resumes a provider/ACP session and returns an opaque external session ref.
4. Coordinator commits `RuntimeBindingId` containing the logical Agent Session,
   Runtime instance, generation, Run, and external reference.
5. Runtime events are accepted only when binding ID, instance ID, generation,
   Run ID, and external scope all match the active record.

### Permission to execution

```text
TaskId + RunId + RuntimeBindingId
              |
              v
RequestId + AcpToolCallRef/provider tool ref
              |
              v
ApprovalId? -> PermitId -> ExecutionId -> evidence/audit refs
```

One tool call may cause zero, one, or several governed executions. Therefore
`ToolUseId` cannot be reused as `ExecutionId`. Repeated execution of one tool
call gets a new Execution ID unless an executor explicitly retries the same
idempotency scope.

## State and sequence semantics

- Task revision is the authoritative per-Task order.
- `MessageId` identifies a command or event and supports deduplication; it does
  not imply order.
- `causation_message_id` points to the direct accepted input that caused an
  event. `correlation.task_id` groups the full lifecycle.
- Runtime generation is a fencing token. Output from an older generation is
  recorded as `stale_runtime_event` diagnostics and cannot mutate Task state.
- A request ID is unique within its protocol connection or COSH Run scope.
  Database uniqueness uses the full scope, not the raw value.
- Channel retries with the same scoped message ref replay admission. A reused
  ref with a different payload digest is a security conflict.
- Actor reassignment, target change, session rebinding, or approval delegation
  is an explicit event; mutation of an existing identity row is forbidden.

## Error and security boundaries

- IDs are validated for prefix, canonical representation, length, and expected
  type before database lookup.
- Authorization always checks Actor-to-Task access and target policy; knowing
  a Task ID or ACP Session ID grants nothing.
- External references are capped in bytes and never used as paths, SQL text,
  log templates, or environment names.
- Tenant/authority is part of channel scope to prevent cross-tenant message ID
  collisions.
- The actor presented by a request cannot override the actor established by
  the authenticated connection.
- `scope_digest` is installation-keyed where linkability outside the
  installation would expose tenant or workspace information.
- Approval and execution accept only the active request digest. Stale or
  replayed permits fail closed.
- Audit additions require schema review; existing v1 readers must not be
  broken by silently adding required identity fields.

## Compatibility and migration

- Keep current `ProviderSessionId` UUID files unchanged and wrap them in
  `ProviderSessionRef` at the bridge boundary.
- Keep existing Shell session, command, Run, request, and tool strings as
  external/legacy references during dual operation.
- Add optional Task, Runtime binding, Approval, Permit, Execution, and Delivery
  correlation only through an audit schema-compatible change or a v2 event
  contract.
- Never backfill guessed Task IDs into legacy audit records. Readers report
  an explicit correlation gap.
- On Gateway adoption, persist a binding event between the new Task and the
  legacy provider session rather than renaming session files.

## Dependencies

- [Protocol contracts](../protocol-contracts/design.md) consumes these
  newtypes and correlation rules.
- [Storage and supervision](../storage-supervision/design.md) persists
  relations and enforces Runtime fencing.
- Phase 1 Gateway API owns authenticated actor admission.
- Phase 1 Broker owns Approval, Permit, and Execution correlation.
- Phase 2 ACP and Shell modules only translate scoped external references.

## Implementation tasks

1. Close the G0 contract-owner and UUID representation ADRs. **Ownership and
   allocator ADR acceptance remain open.**
2. Implement validated internal ID newtypes and bounded external refs.
   **Done for the leaf-contract layer.**
3. Add parent-relation constructors so orphan IDs cannot be serialized by
   ordinary APIs.
4. Add actor resolution and provenance interfaces without transport secrets.
5. Add database constraints and lookup indexes for all scoped identities.
6. Add Runtime generation fencing at event admission.
7. Extend audit through an explicit schema compatibility decision.
8. Publish positive and adversarial identity fixtures.

## Test strategy

- Property tests prove type prefixes never cross-parse and serialization is
  canonical.
- Database tests reject orphan Runs, cross-Task Approvals, reused permits, and
  unscoped external IDs.
- Channel tests replay scoped message IDs and reject cross-tenant collisions or
  changed-payload reuse.
- Runtime tests inject delayed events from a previous generation.
- ACP tests cover numeric versus string JSON-RPC IDs, duplicate tool call text
  across sessions, missing message IDs, and Agent-chosen arbitrary session IDs.
- Security fixtures attempt ID enumeration, log injection, oversized opaque
  values, and actor substitution.

## Open decisions

| Decision | Owner | Must close by |
| --- | --- | --- |
| Accept UUIDv4 allocation or migrate the allocator to UUIDv7 without changing typed prefixes | Contract owners | G0 exit |
| Raw-versus-encrypted storage for channel and ACP external values | Security and storage owners | Before Gateway schema migration 1 |
| Actor lifecycle and local UID remapping policy | Gateway API owner | Phase 1 admission implementation |
| Audit v1 additive fields versus a v2 audit schema | Audit owners | Before first Gateway audit event |
| Whether one ACP Session may bind concurrently to several Tasks | Runtime owner; recommended answer is no in Phase 2 | ACP bridge review |
