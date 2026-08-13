# Phase 0 Identity and Correlation 设计

[English](design.md) | [验收报告](acceptance_zh.md) |
[规划集](../../README_zh.md)

## 状态与决策

- 基线：`up/main` 的 `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`
- 状态：typed identity 基础已实现；G0 storage/admission 退出条件尚未满足

COSH 必须分配自己的 typed lifecycle identity，并把渠道、Shell、cosh-core 与
ACP identifier 保留为有 scope 的 external reference。只有 type 与 scope 都相同
时才能判断 identity 相等。特别需要保持：

```text
TaskId != RunId != AgentSessionId != ShellSessionId
RequestId != ToolUseId != ExecutionId != ApprovalId
```

External session 或 message identifier 可以定位 binding，但不能授权 actor、选择
OS target，也不能成为 Task identifier。

## 目标

- 定义 canonical internal ID、external reference、ownership 与 scope。
- 把一次 user intent 在 channel admission、Task event、Agent Runtime、approval、
  OS execution、audit 与 presentation delivery 之间关联起来。
- 在副作用发生前识别 duplicate delivery、stale Runtime output 与 cross-task substitution。
- 支持 local user、automation 和未来钉钉/飞书 adapter 的 actor resolution，同时
  不在 domain event 中保存渠道 credential。
- 扩展当前 audit 词表，不静默改变它的 v1 wire contract。

## 非目标

- 设计完整 IAM、organization directory、OAuth flow 或 channel login。
- 把 Linux UID 单独作为全局稳定 human identity。
- 把 internal identifier 当作 secret。ID 是不可猜测的 correlation handle，不是
  authorization token。
- 因为文本碰巧相同，就复用 provider、ACP、JSON-RPC 或 Shell ID。
- 在 Phase 0 迁移现有 provider session filename 或 audit event。

## 当前源码证据

| 证据 | 基线事实 | 缺口 |
| --- | --- | --- |
| [`ProviderSessionId`](../../../../../crates/cosh-core/src/session.rs#L23) | Persistence 只接受 canonical lowercase UUID，并把它绑定到 canonical workspace | 它只标识 provider history，不是 Task 或 actor |
| [`AuditIdentity`](../../../../../crates/cosh-shell/src/types/audit.rs#L29) | Audit event 可携带 installation、Shell session、provider session、Run、turn、request、tool 与 command string | 字段是 optional string，并且没有 Task、Approval、Execution、Delivery、actor、target 或 Runtime generation |
| [`ShellEvent`](../../../../../crates/cosh-shell/src/types/mod.rs#L69) | Shell session 和 command identity 与 provider audit identity 分离 | 没有 durable Gateway binding |
| [`AgentRequest`](../../../../../crates/cosh-shell/src/types/mod.rs#L303) 与 [`AgentEvent`](../../../../../crates/cosh-shell/src/types/mod.rs#L402) | `AgentRequest.id` 作为 string Run ID 流入 Shell event | 缺少 type 与 generation fencing |
| [`ProviderToolKey`](../../../../../crates/cosh-shell/src/runtime/provider_tool_state.rs#L236) | In-memory tool correlation 已要求 `(run_id, tool_id)` | Shell 退出后 scope 丢失，且不是 durable execution identity |
| [`RunCommand`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service/command.rs#L21) | Persistent Core service 携带 Run ID 与独立 session scope | Process binding 没有 durable owner/generation record |

实现 worktree 已增加 canonical
[`ids`](../../../../../crates/cosh-gateway-contracts/src/ids.rs)、
[`common`](../../../../../crates/cosh-gateway-contracts/src/common.rs) 与
[`external`](../../../../../crates/cosh-gateway-contracts/src/external.rs) module。
Durable external-reference registry、actor resolver 与 storage constraint 不在此
leaf-contract 切片内。

## Ownership

| Owner | 职责 |
| --- | --- |
| `cosh-gateway-contracts` leaf crate | ID newtype、`Correlation`、`ExternalRef`、constructor validation contract 与 serializable scope type |
| Gateway `IdentityResolver` | 认证 ingress，把 issuer/subject 映射到 `ActorId` 并输出 provenance |
| Task Coordinator | 分配 Task、Run、Approval、Execution、Delivery 与 command Message ID，执行 parent-child invariant |
| Runtime Supervisor | 分配 Runtime instance/generation 与 Connection ID，注册 Agent Session binding |
| Runtime bridge | 把 provider 与 ACP value 保留为 opaque scoped reference，不从中推断语义 |
| Capability Broker | 绑定 actor、Task、Run、target、operation、permit、Approval 与 Execution ID |
| Audit projection | 通过另行评审的 schema revision 添加 optional correlation field，或以 reference 映射 |
| Channel adapter | 构造 bounded issuer-specific external reference 与 idempotency material，不分配 internal ownership |

`cosh-gateway-contracts` 的 G0 ownership ADR 同时管理这些 newtype。`cosh-shell`
保持 standalone 边界，除非后续 ADR 允许直接依赖，否则通过 canonical fixture
mirror Gateway wire ID。

## Identity 分类

### COSH 分配的 internal identity

| Type | Scope 与 lifetime | Parent/invariant |
| --- | --- | --- |
| `InstallationId` | 一个 local Gateway installation，持久 | 不能只从 hostname 或 machine-id 推导 |
| `ActorId` | 本 installation 已知 principal，持久 | 映射自 authenticated `(issuer, subject)` |
| `TaskId` | Durable user intent | Owner 与 target policy context 不可变 |
| `RunId` | 一次 Runtime turn 或 workflow run 尝试 | 只属于一个 `TaskId`；retry 创建新 Run |
| `AgentSessionId` | COSH logical Agent conversation binding | 属于一个 Task context；映射 provider/ACP external session，但不采用其 ID |
| `RuntimeInstanceId` | 一个 supervised child process | 对应一个 launch specification 与 generation sequence |
| `RuntimeBindingId` | Run 与 external Agent Session 的 binding | 绑定一个 Task、Run、Runtime instance、generation 与 external session ref |
| `ApprovalId` | 一个 durable decision request | 绑定一个 Task、Run、request digest 与 policy revision |
| `PermitId` | 一个 Broker authorization result | 绑定所需 Approval、target、operation digest 与 expiry |
| `ExecutionId` | 一次 side effect 尝试 | 绑定一个 permit；只有 executor 定义了幂等 replay 时才能复用 |
| `DeliveryId` | 一个 Outbox event 到一个 sink 的 delivery | 绑定一个 event 与 destination；attempt 单独计数 |
| `MessageId` | 一个 COSH command/event envelope | 在 installation 内全局唯一 |

Internal ID 使用短 type prefix 加 canonical lowercase hyphenated UUIDv4 text，
例如 `tsk_<uuid>` 与 `run_<uuid>`。Allocator 与 workspace 集中的 `uuid` feature
及 Rust 1.88 baseline 保持一致。Durable ordering 仍使用 Task revision 或 database
sequence，绝不使用 UUID ordering。未来可在保持 prefix text contract 不变的前提下
迁移到 UUIDv7，但需要显式 compatibility decision。

### 有 Scope 的 external identity

| Type | 必需 scope | 规则 |
| --- | --- | --- |
| `ChannelConversationRef` | adapter + authority/tenant + conversation | Opaque 且 bounded，不是 actor |
| `ChannelMessageRef` | conversation ref + message value | 提供 ingress deduplication material |
| `ShellSessionRef` | installation + Shell process/session | 本身不能恢复 provider context |
| `ShellCommandRef` | Shell session + command ID | 只标识 PTY evidence |
| `ProviderSessionRef` | Runtime kind + workspace + provider session value | 可映射当前 `ProviderSessionId`，但不是 Task ID |
| `AcpConnectionRef` | Runtime instance + generation | 为 stdio connection correlation 在本地分配 |
| `AcpSessionRef` | ACP connection + opaque Agent session ID | 只能通过显式 binding 跨 Run 复用 |
| `AcpRequestRef` | ACP connection + JSON-RPC ID | JSON-RPC number 与 string 保持不同 wire value |
| `AcpMessageRef` | ACP session + opaque optional message ID | Message ID 缺失时使用 local chunk sequence，不伪造 Agent identity |
| `AcpToolCallRef` | ACP session + opaque tool call ID | 映射一个 internal tool observation，不直接映射 Execution |
| `TerminalRef` | Runtime binding + ACP terminal ID | 只在 terminal ownership record 存在时有效 |

External value 与 scope 分开存储。任何代码都不能通过拼接未经 escape 的 string
创建 composite primary key。

## Typed schema

已提交源码实现下列 shape。以下简化视图省略 helper method 与 validation 细节。

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

`ExternalRef.value` 可能包含私有 tenant 或 user data。除非 protocol continuation
必须使用 raw value，否则 domain 与 audit event 只保存 encrypted reference row ID
或 installation-keyed digest。Log 与 error 只能使用 kind、digest 与 safe suffix。

### Durable relation 草案

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

Foreign key 与 unique constraint 执行 parent relation。Correlation 不能只存在
于 event JSON 中。

## Correlation 传播

### Ingress 到 Task

1. Adapter 校验 transport credential，并构造 issuer、subject、conversation 与
   message reference。
2. `IdentityResolver` 返回 `ActorRef`；失败时在 Task admission 前停止。
3. Gateway 推导或接受 bounded idempotency key。对于 channel message，它是完整
   scoped message ref 的 installation-keyed digest。
4. Coordinator 创建或 replay Task command，并分配 `TaskId` 与 `MessageId`。
5. Raw credential、webhook signature 与 bearer token 在 adapter boundary 外丢弃。

### Task 到 Runtime

1. Coordinator 在 `TaskId` 下创建 `RunId`。
2. Supervisor 选择或启动 `RuntimeInstanceId`，每次新 process 都增加 generation。
3. Coordinator 选择或创建 COSH `AgentSessionId`；Bridge 打开或恢复 provider/ACP
   session，并返回 opaque external session ref。
4. Coordinator 持久化 `RuntimeBindingId`，其中包含 logical Agent Session、Runtime
   instance、generation、Run 与 external reference。
5. 只有 binding ID、instance ID、generation、Run ID 与 external scope 全部匹配
   active record，Runtime event 才会被接受。

### Permission 到 Execution

```text
TaskId + RunId + RuntimeBindingId
              |
              v
RequestId + AcpToolCallRef/provider tool ref
              |
              v
ApprovalId? -> PermitId -> ExecutionId -> evidence/audit refs
```

一个 tool call 可能产生零个、一个或多个 governed execution。因此不能把
`ToolUseId` 复用为 `ExecutionId`。同一个 tool call 的重复 execution 使用新的
Execution ID，除非 executor 明确 retry 同一 idempotency scope。

## 状态与序列语义

- Task revision 是权威 per-Task order。
- `MessageId` 标识 command/event 并支持 deduplication，不表示 order。
- `causation_message_id` 指向直接导致 event 的 accepted input；
  `correlation.task_id` 聚合完整 lifecycle。
- Runtime generation 是 fencing token。旧 generation 的 output 记录为
  `stale_runtime_event` diagnostics，不能修改 Task state。
- Request ID 在对应 protocol connection 或 COSH Run scope 内唯一。Database
  uniqueness 使用完整 scope，不使用 raw value。
- 使用相同 scoped message ref 的 channel retry replay admission。同一 ref 携带
  不同 payload digest 属于 security conflict。
- Actor reassignment、target change、session rebinding 或 approval delegation
  必须形成显式 event，禁止修改已有 identity row。

## Error 与安全边界

- Database lookup 前校验 ID prefix、canonical representation、length 与 expected type。
- Authorization 始终检查 Actor-to-Task access 与 target policy；知道 Task ID 或
  ACP Session ID 不授予任何权限。
- External reference 有 byte 上限，不能用作 path、SQL text、log template 或
  environment name。
- Channel scope 包含 tenant/authority，防止跨 tenant message ID collision。
- Request payload 提供的 actor 不能覆盖 authenticated connection 确立的 actor。
- 如果跨 installation linkability 会暴露 tenant 或 workspace 信息，`scope_digest`
  必须使用 installation-keyed digest。
- Approval 与 execution 只接受 active request digest；stale 或 replay permit fail closed。
- Audit 扩展需要 schema review；不得通过静默新增 required identity field 破坏 v1 reader。

## 兼容与迁移

- 保持当前 `ProviderSessionId` UUID file 不变，在 bridge boundary 把它包装成
  `ProviderSessionRef`。
- Dual operation 期间，把现有 Shell session、command、Run、request 与 tool string
  保留为 external/legacy reference。
- Task、Runtime binding、Approval、Permit、Execution 与 Delivery correlation 只能
  通过 audit schema-compatible change 或 v2 event contract 添加。
- 绝不向 legacy audit record 回填猜测的 Task ID。Reader 要报告显式 correlation gap。
- Gateway 启用时，持久化新 Task 与 legacy provider session 的 binding event，
  不重命名 session file。

## 依赖

- [Protocol Contracts](../protocol-contracts/design_zh.md)消费这些 newtype 与
  correlation rule。
- [Storage and Supervision](../storage-supervision/design_zh.md)持久化 relation 并执行
  Runtime fencing。
- Phase 1 Gateway API 拥有 authenticated actor admission。
- Phase 1 Broker 拥有 Approval、Permit 与 Execution correlation。
- Phase 2 ACP 与 Shell module 只转换 scoped external reference。

## 实施任务

1. 关闭 G0 contract-owner 与 UUID representation ADR。**Ownership 与 allocator
   ADR acceptance 尚未完成。**
2. 实现 validated internal ID newtype 与 bounded external ref。
   **Leaf-contract layer 已完成。**
3. 添加 parent-relation constructor，使普通 API 无法序列化 orphan ID。
4. 添加不包含 transport secret 的 actor resolution 与 provenance interface。
5. 为所有 scoped identity 添加 database constraint 与 lookup index。
6. 在 event admission 中添加 Runtime generation fencing。
7. 通过显式 schema compatibility decision 扩展 audit。
8. 发布 positive 与 adversarial identity fixture。

## 测试策略

- Property test 证明不同 type prefix 不能 cross-parse，serialization 为 canonical。
- Database test 拒绝 orphan Run、cross-Task Approval、reused permit 与 unscoped
  external ID。
- Channel test replay scoped message ID，并拒绝 cross-tenant collision 或 payload
  已改变的 reuse。
- Runtime test 注入 previous generation 的 delayed event。
- ACP test 覆盖 numeric/string JSON-RPC ID、跨 session 重复 tool call text、缺失
  message ID 与 Agent 自选 arbitrary session ID。
- Security fixture 尝试 ID enumeration、log injection、oversized opaque value 与
  actor substitution。

## 开放决策

| 决策 | Owner | 最晚关闭时间 |
| --- | --- | --- |
| 接受 UUIDv4 allocation，或在不改变 typed prefix 的前提下迁移 allocator 到 UUIDv7 | Contract owner | G0 退出前 |
| Channel 与 ACP external value 使用 raw storage 还是 encrypted storage | Security 与 storage owner | Gateway schema migration 1 前 |
| Actor lifecycle 与 local UID remapping policy | Gateway API owner | Phase 1 admission 实现前 |
| Audit v1 additive field 或 v2 audit schema | Audit owner | 第一个 Gateway audit event 前 |
| 一个 ACP Session 是否允许同时绑定多个 Task | Runtime owner；Phase 2 建议不允许 | ACP bridge review |
