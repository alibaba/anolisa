# Phase 0 Protocol Contracts 设计

[English](design.md) | [验收报告](acceptance_zh.md) |
[规划集](../../README_zh.md)

## 状态与决策

- 基线：`up/main` 的 `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`
- 状态：leaf contract 基础已实现；Phase 0 freeze 尚未关闭
- ACP profile：wire protocol v1，`initialize.protocolVersion = 1`

Side-effect-free
[`cosh-gateway-contracts`](../../../../../crates/cosh-gateway-contracts/src/lib.rs)
crate 已实现第一批 COSH 自有 domain contract。ACP type 只能留在 ACP bridge 内，
现有 cosh-core JSONL message 只能留在 CoshCore bridge 内。Task Execution Plane
只使用本文定义的中立 command、event、runtime message 和 capability request。
完整 schema/golden-fixture corpus、reducer property matrix、integrated coordinator
port 与 bridge 仍属于 Phase 0/1 工作。

ACP protocol version 与 SDK package version 相互独立。候选 Bridge 协商 ACP protocol
`1`，准确固定官方 SDK 2.0.0，并把 cosh-ng Rust/RPM baseline 提升到 1.88。
绝不能从 wire version 推断 SDK package major。

## 目标

- 冻结 Phase 1 和 Phase 2 共用的 Task、Runtime、Capability、Approval 与
  presentation event envelope。
- 防止 ACP、cosh-core、Shell、HTTP、钉钉或飞书 payload 成为持久 domain object。
- 在存储代码出现前定义 command idempotency、event ordering、cancellation、
  terminal outcome 与 error semantics。
- 保留当前 `cosh-shell` 独立 crate 边界。
- 提供各 bridge 可以独立验证的 schema 与 golden fixture。

## 非目标

- 实现 Gateway daemon、database、Runtime Supervisor 或 ACP bridge。
- 在 Phase 0 替换现有 Shell/Core control protocol。
- 标准化远端 ACP transport。Phase 2 只使用本地 stdio。
- 定义 provider prompt、model API、OS policy rule 或渠道专用 authentication payload。
- 承诺外部副作用 exactly-once。契约提供持久 admission 与幂等 execution identity，
  executor 仍要自行证明 replay behavior。

## 当前源码证据

| 证据 | 基线事实 | 契约含义 |
| --- | --- | --- |
| [`CONTROL_PROTOCOL_VERSION`](../../../../../crates/cosh-core/src/protocol.rs#L9) 与 [`InputMessage`](../../../../../crates/cosh-core/src/protocol.rs#L60) | cosh-core 接收 exact version 为 `1` 的产品专用 JSONL protocol | 它不是 ACP v1，必须留在 `CoshCoreBridge` 后 |
| [`OutputMessage`](../../../../../crates/cosh-core/src/protocol.rs#L202) 与 [`CoreControlRequest`](../../../../../crates/cosh-core/src/protocol.rs#L360) | streaming、approval、question 与 Shell evidence 使用 Core 专用 shape | 必须由 bridge 转换，这些 type 不得进入 Task storage |
| [`AgentAdapter`](../../../../../crates/cosh-shell/src/adapter/mod.rs#L87) 与 [`AgentEvent`](../../../../../crates/cosh-shell/src/types/mod.rs#L402) | 已有可用的 lifecycle boundary，但 ID 和 payload 是 Shell 自有 string | 中立 Runtime Port 复用语义，不复用 Rust type |
| [`PersistedSession`](../../../../../crates/cosh-core/src/session.rs#L83) | Provider conversation history 已有 versioned envelope | Provider Session 与持久 Task、Event contract 保持分离 |
| [`AuditIdentity`](../../../../../crates/cosh-shell/src/types/audit.rs#L29) | Audit event 已有跨 runtime correlation field | Phase 0 identity module 扩展并类型化该词表 |
| [Runtime contract](../../../../../docs/design/runtime-contracts.md) | 当前 ownership 与 negotiation 已按实现记录 | 迁移必须在 bridge 通过验收前保持该路径可用 |

实现 worktree 已增加中立 leaf type，但仍不存在 ACP dependency、
`TaskCoordinator`、Task event store、Outbox 或 behavioral Runtime Port。

## 模块 Ownership

| Owner | 规划职责 | 明确排除 |
| --- | --- | --- |
| `cosh-gateway-contracts` leaf crate | 纯 serializable ID、command/event envelope、error code 与 enum；schema fixture 尚未完成 | I/O、async trait、ACP SDK type、database record |
| 未来的 `cosh-gateway::ports` | `TaskStore`、`AgentRuntimePort`、`CapabilityBrokerPort` 与 `PresentationPort` behavioral trait | Transport parsing 与 provider 专用逻辑 |
| `cosh-gateway` coordinator | 校验 command、执行 state transition、追加 event 并调用 port | 直接解析 ACP 或 cosh-core JSONL |
| `CoshCoreBridge` | 在中立 Runtime message 与现有 JSONL protocol 之间转换 | 拥有 Task state 或 authorization decision |
| `AcpClientBridge` | 把 ACP v1 SDK type 和 callback 转换为中立 Runtime、Capability contract | 承载渠道流量或持久化 ACP wire message |
| `cosh-shell` attachment | mirror 公共 Gateway wire contract，并用共享 fixture 验证 | 依赖内部 crate，或迁移后监督 Agent process |

`cosh-gateway-contracts` 已作为 side-effect-free leaf crate 实现，它不等同于现有
`cosh-types` 的一个 module。G0 仍需通过 ownership ADR，并补齐 versioned schema/
fixture。本文冻结 ownership 和 dependency direction：pure
type 指向内层，transport 与 bridge 指向 pure type，domain crate 不依赖 SDK。该
选择不会改变 `cosh-shell` 的 standalone/no-internal-dependency 规则；首版 Shell
attachment 通过 Gateway wire client/mirror 与 canonical fixture 保持一致。若要直接
依赖该 leaf crate，必须另行通过 ADR。

## Contract 分层

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

只有 Task event 是 lifecycle 的权威事实。Runtime update 与 presentation event
必须先由 coordinator 校验并记录，才能成为持久事实。

## Typed contract

已提交的模块包括
[`task`](../../../../../crates/cosh-gateway-contracts/src/task.rs)、
[`runtime`](../../../../../crates/cosh-gateway-contracts/src/runtime.rs)、
[`capability`](../../../../../crates/cosh-gateway-contracts/src/capability.rs) 与
[`error`](../../../../../crates/cosh-gateway-contracts/src/error.rs)。以下简化定义描述
稳定语义，字段级细节以已提交的 Rust type 为准。所有 ID 都是 identity module
中的 validated newtype。

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
    Prompt { run_id: RunId, input: Vec<ContentPart> },
    ResolvePermission { request_id: RequestId, decision: RuntimePermissionDecision },
    Cancel { run_id: RunId, cause: CancelReason },
    Close { binding: RuntimeBindingRef },
}

enum AgentRuntimeEvent {
    SessionOpened { binding: RuntimeBindingRef },
    MessageChunk { message_id: RuntimeMessageId, content: ContentPart },
    ToolCallObserved { tool_use_id: ToolUseId, summary: ToolSummary },
    PermissionRequested { request: CapabilityRequest },
    UsageUpdated { usage: RuntimeUsage },
    Completed { outcome: RunOutcome },
    TransportFailed { error: RuntimeError },
}
```

必须提供 `cosh.gateway.command`、`cosh.task.event`、`cosh.runtime.command` 和
`cosh.runtime.event` wire schema，并全部从 schema version `1` 开始。Schema
version 与 ACP `protocolVersion` 无关。

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

Permit 必须绑定 target、规范化 operation digest、发起请求的 Run 与 expiry。
Bridge 不能创建 permit。ACP `session/request_permission` 要先转换成该 request，
只有 Broker decision 才能转换回 ACP response。

`operation_digest` 覆盖完整 canonical namespace、operation name 与 normalized
arguments。`OperationDescriptor.arguments_digest` 只是更窄的 policy detail，不能用作
permit authority。Trusted admission 负责 canonicalization 与 hashing，并在 Broker
policy evaluation 前 pin target、完整 descriptor、operation digest 与 requested scope。

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

Raw provider error、stderr、secret、prompt 与 stack trace 都不得进入
`safe_message`。详细诊断通过 bounded、redacted evidence 暴露。

## 状态与序列语义

### Command admission

1. 在构造 domain command 前解析并认证 `ActorRef`。
2. 查询 `(actor, idempotency_key, command_kind)`。
3. 相同 payload digest 已被接受时，返回原结果。
4. 同一 key 对应另一 digest 时，返回 `idempotency_conflict`。
5. 如果提供 `expected_task_revision`，必须进行校验。
6. 在同一个 storage transaction 中追加 Task event、更新 projection 并加入
   Outbox entry。
7. Transaction commit 后才能调用 Runtime 或 presentation work。

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

- 一个 Run revision 只能接受一个 terminal Run event。
- 迟到的 Runtime update 作为诊断保留，但不再应用到 terminal projection。
- Runtime session ID 与 JSON-RPC request ID 都是有 scope 的 opaque reference，
  二者都不能确定 Task identity。
- Transport cancellation 开始前，cancellation request 必须先持久化。如果
  completion 赢得竞争，保留 completion；否则 coordinator 记录最终 cancellation stage。
- Event cursor 指向 durable Task sequence，不指向 ACP notification order 或
  process-local channel offset。

## ACP v1 Compatibility Profile

Phase 2 必须实现稳定 v1 baseline method `initialize`、`session/new`、
`session/prompt`、`session/cancel` 与 `session/update`。可选 lifecycle、content、
filesystem、terminal、elicitation 与 config feature 只能在协商成功时启用。
Bridge 必须做到：

- 发送 `protocolVersion: 1`；Agent 选择不支持的版本时 cleanly close；
- 把未提供的 capability 视为 unsupported；
- 通过 stdin/stdout 使用本地 newline-delimited JSON-RPC；
- 把 ACP Session ID、JSON-RPC ID、message ID 与 tool call ID 保持为 opaque
  external reference；
- 把 `session/request_permission`、`terminal/*` 与 `fs/*` 转换到 COSH governance，
  protocol reader 不得直接执行；
- 实现 `session/cancel` 语义和经过 capability gate 的 `$/cancel_request`，且不把
  transport cancellation 等同于 durable Task completion；
- 把仍处于草案状态的 ACP Streamable HTTP transport 排除在 Phase 0-2 contract 外。

规范资料：[ACP v1 initialization](https://agentclientprotocol.com/protocol/v1/initialization)、
[prompt turn](https://agentclientprotocol.com/protocol/v1/prompt-turn)、
[cancellation](https://agentclientprotocol.com/protocol/v1/cancellation) 与
[transports](https://agentclientprotocol.com/protocol/v1/transports)。

## Error 与安全边界

- 在 state mutation 前拒绝未知 schema version。
- 未知 enum value 默认拒绝；只有明确声明 forward-compatible 的字段才能作为
  bounded opaque metadata 保留。
- Deserializer 必须限制 byte、collection、nesting 与 text size。
- 外部 path 在 Capability Broker 中 canonicalize；ACP message 中的 path 不构成授权。
- 渠道 actor 不能提供 internal ID、ownership、policy result 或 event revision。
- Runtime update 在与 active fenced Runtime binding 关联前都属于不可信输入。
- Approval response 必须通过 actor authorization 并匹配准确的 pending
  `ApprovalId`；仅有 JSON-RPC response 不足以授权执行。
- Durable contract 禁止 secret 与完整 environment map。

## 兼容与迁移

1. 添加 type、schema 与 fixture，不改变当前 JSONL behavior。
2. 用 `CoshCoreBridge` 包装当前 `AgentAdapter` 和 cosh-core JSONL 路径，并输出
   中立 Runtime event。
3. 新 bridge conformance fixture 与当前 protocol test 并行运行。
4. Phase 1 验收完成后，才把 durable ownership 移交 Gateway。
5. Phase 2 把 Shell 迁移为 attachment mode；兼容窗口内显式保留当前 local runtime
   fallback。
6. 删除或改变 v1 field 必须通过 schema-v2 决策与 migration fixture；新增 optional
   field 保持 v1 compatibility。

本规划不重写现有 `ProviderSessionId` file、audit segment 或 Shell/Core control
protocol version `1`。

## 依赖

- [Identity and Correlation](../identity-correlation/design_zh.md)冻结 ID constructor、
  scope 与 inheritance。
- [Storage and Supervision](../storage-supervision/design_zh.md)冻结 atomic persistence
  与 child-process ownership。
- Phase 1 Gateway、Task Plane、Broker 与 CoshCore Bridge 消费这些 contract。
- Phase 2 ACP、Shell 与 Web module 只提供 transport adapter。

## 实施任务

1. 在选定的 contract owner 中添加 pure newtype 与 envelope。**已完成。**
2. 为每一种 message 发布 JSON Schema 与 canonical JSON fixture。
3. 添加 bounded deserialization 与 stable error code。**Scalar value 与 error 已
   bounded；aggregate collection total 仍未完成。**
4. 只用中立 type 定义 port trait。
5. 添加 state-machine reducer，并用 property test 验证 terminal uniqueness 与
   revision monotonicity。**Critical reducer test 已存在；property matrix 尚未完成。**
6. 针对现有 JSONL protocol 添加 CoshCore translation fixture。
7. 添加由 pinned official SDK 生成的 ACP v1 fixture。
8. 添加 compatibility manifest，分别记录 product schema、ACP protocol、SDK 与
   cosh-core control version。

## 测试策略

- Schema test 校验所有 golden example，并拒绝未知 required field、oversized input
  与错误 ID scope。
- Round-trip test 覆盖 JSON serialization，不依赖 field order。
- State-machine property test 组合 duplicate、delayed、cancelled 与 terminal event。
- Bridge contract test 不调用真实 model，也不修改 host。
- ACP conformance 使用通过 stdio 运行的 deterministic fake Agent。
- Security test 尝试 cross-task ID substitution、permit replay、stale Runtime
  generation、path escape 与包含 secret 的 error。

## 开放决策

| 决策 | Owner | 最晚关闭时间 |
| --- | --- | --- |
| 通过 ADR 接受已实现的 `cosh-gateway-contracts` ownership，并补齐 v1 schema/fixture | cosh-ng maintainers | G0 退出前 |
| UUIDv4 到 UUIDv7 的 allocator migration 与兼容性 | Identity module | Identity contract review |
| JSON v1 之外的公共 Gateway wire encoding | Gateway API module | Phase 1 API freeze |
| 首批 conformance profile 包含哪些已稳定 ACP optional capability | ACP bridge owner | Phase 2 开始实现前 |
| Shell fallback 兼容窗口长度 | Product 与 runtime owner | Phase 2 rollout review |
