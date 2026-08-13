# Phase 1 Capability Broker 设计

[English](design.md) | [验收报告](acceptance_zh.md)

## 状态与决策

本文基于上游提交 `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`。第一版 process-local 纯逻辑
切片已经实现，但不构成完整 Phase 1 安全声明。`CapabilityBroker` 是所有 OS side effect 必须经过的
policy enforcement 和 permit
authority，无论 intent 来自 Gateway、cosh-core、ACP、local model、Shell、Skill、MCP 或 workflow。
Execution target 只有收到绑定 target 和 operation 的有效 permit 才能执行。

Policy 要求时 approval 是必要条件，但 approval 本身不是 executable authority。Committed approval
只能允许 Broker 签发范围更窄的 permit，不能扩大 actor、target、action、resource、lifetime 或执行次数。

## 已实现的 process-local 切片

Gateway 现在包含 provider-neutral
[`PolicyPort`](../../../../../crates/cosh-gateway/src/capability/broker.rs)、
[`PermitStore`](../../../../../crates/cosh-gateway/src/capability/broker.rs) 与
`CapabilityBroker`。Authorization 在调用 policy 前校验 request expiry、准确的 Task/Run parent、
完整 authenticated `ActorRef` 与 `AuthoritativeRequestBinding`。该 binding pin exact target、完整
`OperationDescriptor`、完整 operation digest 与 requested scope。因此 Actor provenance 或 request
content substitution 不能影响 policy。Policy 可以 deny、require approval 或 allow。Revision 为零和
已经过期的 policy authority 会 fail closed。

Direct allowance 签发绑定 actor、Task、Run、Execution、exact `TargetRef`、完整 canonical
operation digest、policy revision、expiry 与一次使用的 `ExecutionPermit`。Operation digest
覆盖 namespace、name 与 normalized arguments；`arguments_digest` 只作为较窄的 policy detail。
Canonicalization 与 hashing 由 trusted ingress 负责，Broker 不使用 argument-only digest 签发
authority。Process-local
[`MemoryPermitStore`](../../../../../crates/cosh-gateway/src/capability/memory.rs) 在同一个 mutex 内校验
这些字段并标记 consumed，因此并发 caller 只有一个可以 claim。Binding check 失败不会 consume
authority。

该切片在 effect 前终止。`MemoryPermitStore` 不持久，进程退出会丢失状态。Approval 只产生
`ApprovalRequest`；当前没有 durable approval ledger、resolution lookup 或基于 approval 的
re-authorization，direct allow permit 的 `approval_id = None`。Immutable target resolution、
lease/runtime binding、durable permit/execution state、audit gate、revocation、OS execution、
network/ACP API、crash recovery 与 reconciliation 均未实现。

## 目标

- 将全部 side-effect intent 规范化为一个 typed `CapabilityRequest`。
- 同时评估 actor、Task、Run、target、operation、resource scope、risk 和 policy revision。
- 返回稳定 denial、持久 approval specification 或短生命周期 target-bound permit。
- 将每个允许的 effect 绑定到一个 `ExecutionId` 和 replay-safe execution ledger。
- Opaque Shell command fail closed，并优先使用 deterministic typed operator。
- 将 Task control event 与现有 unified security audit contract 关联。
- 在不降低 target 或 approval 检查的前提下保留安全 local/offline policy path。

## 非目标

- 拥有 Task aggregate、channel approval UI、Agent lifecycle、PTY rendering 或 provider session。
- 把 `cosh audit check`、approval callback、model tool name 或 policy decision 当作 permit。
- 保证跨进程或机器 crash 的 exactly-once effect。
- 因 caller 在本机就授予宽泛 shell、root、filesystem 或 network access。
- 在 Phase 0 target identity 决策接受前支持 remote attestation。
- 将任意 natural language 解析成 OS authority。

## 当前源码证据

| `6c115aef` 的证据 | 可复用行为 | 安全缺口 |
| --- | --- | --- |
| [`cosh-types/audit/event.rs`](../../../../../crates/cosh-types/src/audit/event.rs) | Typed `Action`、policy `Decision`、audit identity、versioned event 与 redaction shape。 | 无 capability request、Execution ID、target identity 或 permit。 |
| [`cosh-platform/audit/evaluate.rs`](../../../../../crates/cosh-platform/src/audit/evaluate.rs) | Deterministic first-match PDP，输出 allow/deny/require-approval。 | Policy result 不是 execution authority。 |
| [`cosh-platform/audit/action.rs`](../../../../../crates/cosh-platform/src/audit/action.rs) | Shell action parsing 拒绝不支持的 compound/metacharacter shape。 | 只覆盖 command-policy classification，不提供完整 target binding。 |
| [`cosh-platform/audit.rs`](../../../../../crates/cosh-platform/src/audit.rs) | 已有 policy check 和 security-boundary audit segment write。 | 无 permit ledger 或 consume protocol。 |
| [`cosh-core/core.rs`](../../../../../crates/cosh-core/src/core.rs) | 集成 hook/policy decision、approval、audit event 与 tool execution。 | Allowed tool 在 core 内执行，approval 不经过公共 Broker。 |
| [`cosh-core/protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs) | `can_use_tool` 携带 tool、input、tool-use ID、hook flag 与 audit reference。 | 缺少 Task/Run/target/Execution ID 和 permit。 |
| [`cosh-platform/pkg.rs`](../../../../../crates/cosh-platform/src/pkg.rs)、[`svc.rs`](../../../../../crates/cosh-platform/src/svc.rs) 与 [`checkpoint.rs`](../../../../../crates/cosh-platform/src/checkpoint.rs) | 已有 typed OS operation 与部分 dry-run path。 | Caller 无公共 permit verifier 即可调用。 |

基线上 `cosh-cli`、`cosh-core` 和 Shell execution path 都可能在没有 target-bound Broker permit 的情况下
触达副作用。现有 policy 与 audit module 是基础，不证明 Broker 已经存在。

## Ownership 与 ports

```mermaid
flowchart LR
    AR["AgentRuntimePort"] --> BP["CapabilityBrokerPort"]
    GW["Gateway direct operation"] --> BP
    BP --> BR["CapabilityBroker"]
    BR --> PDP["PolicyDecisionPort"]
    BR --> AP["ApprovalReadPort"]
    BR --> PL[("Permit / execution ledger")]
    BR --> AU["AuditPort"]
    BR --> ET["ExecutionTargetPort"]
    ET --> V["PermitVerifier"]
    V --> OP["Typed operator / Shell executor"]
    OP --> OS["Bound GuestOS target"]
    BR --> TR["BrokerResultPort"]
    TR --> TC["TaskCoordinator\nTask 唯一 writer"]
```

当前切片实现同步的 `PolicyPort::evaluate`、`PermitStore::issue`、`PermitStore::consume`，以及
`CapabilityBroker::authorize` 和 `CapabilityBroker::claim`。概念模型中的 `execute`、`reconcile`、
approval-read、audit 与 target port 仍是未来边界。Broker 不依赖 Task storage 或 executor。

Broker 拥有 capability normalization、policy orchestration、permit issuance、permit consumption state 和
execution correlation。Execution adapter 拥有 last-mile operation，并在使用前立即验证 permit。
`TaskCoordinator` 拥有 approval state 和 Task event；Broker 只提交 result，不能写 Task storage。

概念 ports 如下：

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

中立 ID 与 wire DTO 已位于 side-effect-free `cosh-gateway-contracts` leaf。Policy adapter 可以复用
`cosh-types` audit type，但不能把 Task/Gateway contract 移入 `cosh-types`。

## Capability request schema

已实现 leaf request 包含 request、Task、Run 与 Actor identity、`TargetRef`、由
namespace/name/arguments-digest 组成的 operation descriptor、独立的完整 canonical operation digest、
requested resource/access scope、input digest 与 expiry。Trusted ingress 必须在构造 request 和独立
`AuthoritativeRequestBinding` 前 canonicalize 并 hash 完整 operation。`RequestContext` 提供 current
time、parent binding 与 authoritative target/descriptor/digest/scope。Broker 在 policy 前逐项比较，
不从 presentation field 重建 authority。下面的扩展
target schema 仍是目标架构；runtime principal、lease fence、effect classification、typed operation
variant 与 prior approval correlation 尚未进入第一版切片。

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

`CapabilityOperation` 是 supported operation 的 closed、versioned enum：

```text
FileRead, FileWrite, DirectoryList, ProcessInspect, ProcessSignal,
PackageQuery, PackageInstall, PackageRemove,
ServiceQuery, ServiceStart, ServiceStop, ServiceRestart,
CheckpointList, CheckpointCreate, CheckpointRestore,
NetworkConnect, ShellCommand, PtyAttach, SkillInvoke, McpToolInvoke
```

每个 variant 携带 typed field 与显式 limit。Unknown operation 返回 `unsupported_capability`，不能 fallback
为 `ShellCommand`。Raw string 可以作为有界 audit display data 保留，但只有经过专用 parser 规范化后才能
参与 policy matching。

Effect class 为 `Observe`、`WorkspaceWrite`、`HostMutation`、`PrivilegedMutation`、
`ExternalNetwork` 和 `InteractiveControl`。Classification 是风险下限；policy 可以提高风险，但不能把 typed
operation 降到 built-in minimum 以下。

## Target identity

`TargetRef` 只供用户选择，不能写入 permit。Policy evaluation 前，`TargetResolver` 将其 pin 为不可变
`TargetIdentity`：

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

Local target identity 由 daemon installation、pinned workspace/namespace、machine/boot identity 和 effective
credential 得出。Remote GuestOS target 必须由 Phase 0 定义 authenticated instance/agent epoch 与 replay
resistance。Hostname、IP、display label、workspace path string、channel installation 或 caller 提供的 instance
ID 都不充分。

Authorization 后 target 发生变化会使 decision 失效。相关场景下 symlink、mount namespace、container、
UID、boot、agent epoch 和 workspace-root 变化都属于 target revalidation。

当前 permit 绑定 exact `TargetRef`，可以拒绝直接 target substitution，但不能提供 immutable target
identity 或 attestation。在该缺口关闭前不连接 OS executor。

## Decision 与 approval flow

当前切片只实现前三种结果，也就是 deny、approval request 与 permit。Approval resolution 和
re-authorization 未实现，所以下述后续 approval 步骤仍是设计要求。

Broker 返回以下之一：

```text
Denied { reason_code, policy_revision }
ApprovalRequired { approval_spec, operation_digest, target_digest, expires_at }
Permitted { permit }
AlreadyExecuting { execution_id, status }
ReconciliationRequired { execution_id, reason_code }
```

流程如下：

1. 校验 schema、actor/runtime principal、Task/Run binding、lease fence、deadline 与 limit。
2. Resolve 并 pin `TargetIdentity`，canonicalize operation 和 resource scope。
3. 计算 operation/target digest，再评估 built-in risk floor 与 loaded policy。
4. 持久并 audit denial，或把 `ApprovalRequired` 返回给 `TaskCoordinator`。
5. Coordinator commit `ApprovalRequested`，presentation 异步投递。
6. Coordinator commit 第一个有效 resolution，并携带 `ApprovalId` 与 approval revision 重新提交同一
   capability request。
7. Broker 读取并验证 resolution，重新解析 target 和 policy，再签发不比 approved specification 更宽或
   更长的 permit。
8. Execution 通过 ledger consume permit，并调用 target adapter。

Approval 与 permit issuance 之间 policy 或 target 变化时必须重新评估。更严格结果会 deny 或请求新
approval；approval 不能跨 widened scope 沿用。

## Permit contract

已实现 `ExecutionPermit` 绑定 permit/request/execution ID、actor、Task、Run、exact target、
完整 operation digest、policy revision、optional approval ID、expiry 与 `single_use = true`。它尚未携带
immutable target identity、runtime/lease fence、durable issuance timestamp、revocation state 或
cross-process integrity proof。

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

Phase 1 local execution 应使用 opaque ledger-backed permit handle 加 integrity proof，避免 self-contained
宽泛 bearer token。`PermitVerifier` 校验全部字段、当前 target identity、expiry、fence 与 ledger state。
Permit serialization 有界，不包含 raw command、secret、output 或 credential value。

约束如下：

- 一个 permit 映射一个 `ExecutionId`、一个 exact operation digest 和一个 target digest；
- Permit 不能跨 actor、Task、Run、Runtime、target、workspace 或 boot 转移；
- Used、expired、revoked、stale-fence、malformed 或 unknown permit fail closed；
- 不支持 permit renewal；fresh request 和 current policy 生成新 permit；
- Approval 可以缩小 requested operation，但不能签发 wildcard permit；
- Target adapter 不接受无 permit 的 typed operation 或 raw shell fallback。

## Transaction、idempotency 与 execution ledger

Authorization 按 `(TaskId, RunId, RequestId, operation_digest, target_digest)` 去重。Retry 返回原 denial、
approval specification 或仍有效且未 consume 的 permit。同一 `RequestId` 用于另一 digest 时返回
`idempotency_conflict`。

Permit issuance 原子记录 permit metadata、`ExecutionId`、policy/approval reference、expiry 和 `Ready`
status。Authority 可用前必须持久化 security-boundary audit；audit failure 会 deny issuance。

Execution 使用 permit ID、fence 和 target executor claim 原子执行 `Ready -> Claimed`。副作用前记录
`Started` audit evidence。Target 返回 typed result 与 reconciliation evidence；ledger 转为 `Succeeded`、
`Failed` 或 `Uncertain`。重复 execute call 返回 stored terminal result 或 `execution_in_progress`，不能创建
另一个 effect。

在 `Claimed` 或 `Started` 后 crash 可能使 effect unknown。Recovery 调用
`ExecutionTargetPort.reconcile(ExecutionId)`，不能把 permit reset 为 `Ready`。Target 无法证明 terminal
result 时，status 变为 `Uncertain`，Task suspend，并要求 operator-safe reconciliation decision。

## Shell 与 typed operator 规则

优先使用 typed `cosh-platform` operation，因为 action 和 resource field 可以精确 binding。现有
`cosh-cli` 是 user-facing envelope；Broker 应调用 typed platform adapter 或窄 operator protocol，不能
解析任意 CLI output 推断 authority。

`ShellCommand` 是例外 operation：

- Classification 前 tokenize，并支持 tab/newline separator；
- 拒绝 shell metacharacter 和 compound/unspaced variant，除非 isolated、explicit high-risk executor
  contract 明确支持；
- 绑定 exact argv、executable identity、cwd/workspace identity、选中的 environment name、UID、timeout、
  output budget 和 target；
- 不允许 prefix、free-form continuation 或 inherited interactive shell permit；
- Interactive ownership 必须使用独立 `PtyAttach` permit；
- Parsing、executable resolution、target pinning 或 policy classification 不完整时 fail closed。

Brokered cosh-core profile 中，direct side-effecting core tool 必须禁用或 delegated。现有 host-executed
shell response 只有在 Bridge 获得 permit 且 execution target 返回 evidence 后才可使用，不能作为 blanket
approval response。

## Security audit 与 Task correlation

Task event 与 security audit event 保持分离。Broker 为 request、policy result、approval correlation、permit
issuance/denial/revocation、execution start、terminal result 与 uncertainty 生成 audit event。Event 携带有界
`TaskId`、`RunId`、`RequestId`、`ToolUseId`、`ExecutionId`、policy revision、target digest、result code、
duration 与 redaction status。

Sensitive value 使用 digest 或 opaque evidence reference。Permit issuance 与 execution start 必须使用现有
audit store 的 security-boundary durability behavior。Best-effort audit mode 不能在 Broker path 授权
privileged mutation。

## Error model

稳定分类包括 `invalid_capability`、`unsupported_capability`、`forbidden`、`approval_required`、
`approval_invalid`、`approval_expired`、`target_unresolved`、`target_changed`、`policy_changed`、
`idempotency_conflict`、`permit_expired`、`permit_revoked`、`permit_consumed`、
`permit_scope_mismatch`、`stale_lease`、`audit_unavailable`、`execution_in_progress`、
`execution_uncertain`、`target_unavailable` 和 `internal`。

Error 区分 safe same-request retry、new authorization、new approval、target reconciliation 和 non-retryable
denial，不能回显 secret input 或无界 target output。Transport timeout 不能证明 effect 没有发生。

## 迁移与兼容

1. 固化 Phase 0 capability、target identity、permit、audit correlation 与 approval schema。
2. 引入 policy boundary 与 in-memory permit ledger。**Pure logic 已实现；fake target 与 production
   policy adapter 尚未完成。**
3. 增加 persistent permit/execution ledger 和 required audit boundary。
4. 新 Gateway typed operation 通过 Broker；旧 direct CLI 作为 opt-in legacy 暂时保留。
5. 增加 brokered `CoshCoreBridge` profile，禁用或 delegated direct side-effecting core tool。
6. Shell/ACP/Skills/MCP path 只有在 adapter coverage 完整后才接入。
7. Parity 与 recovery acceptance 通过后，删除或显式隔离 legacy bypass。

Rollback 禁用 brokered mode 并保留现有 binary，但同时会移除新安全保证。只要 enabled production
adapter 仍有 direct bypass，release 就不能宣传“all side effects governed”。

## 依赖

- Phase 0 identity、target、capability、schema compatibility、storage、secret 与 threat-model 决策。
- [Task Execution Plane](../task-execution-plane/design_zh.md)：Task/Run state、durable approval 与 result
  recording。
- [Gateway API](../gateway-api/design_zh.md)：actor 与 direct-operation ingress。
- [Cosh Core Bridge](../cosh-core-bridge/design_zh.md)：JSONL tool-intent translation 与 brokered runtime
  profile。
- `cosh-platform` typed operation 和 audit policy/storage 继续作为 implementation foundation。

## 实现任务分解

1. 定义 capability、target、approval reference、permit 与 execution result schema。
2. 实现 target resolution/pinning 与 canonical operation/resource digest。
3. 使用 built-in minimum effect classification 适配当前 audit policy evaluation。
   **当前只有 neutral policy port。**
4. 实现 decision flow 与 durable approval correlation，但不能写 Task。
   **Branching 已实现；durable resolution 与 re-authorization 尚未完成。**
5. 实现 permit issuance、verification、revocation、consume 与 execution ledger。
   **Process-local issue/claim 已实现；durability、revocation 与 execution lifecycle 尚未完成。**
6. 实现 typed local target adapter 和严格 Shell command/Pty path。
7. 增加 required security audit event 与 Task correlation reference。
8. 集成 Gateway 与 `CoshCoreBridge`，再接 Phase 2 ACP 和 presentation path。
9. 增加 bypass inventory 和 build-time dependency/coverage check。

## 测试策略

当前八个 unit test 覆盖 request expiry 与 parent substitution、deny 与 approval branch、policy failure 与
invalid authority、完整 permit binding、binding mismatch 不消费、expiry/replay，以及八路 concurrent
claim。仍需更广的 security suite：

- Stable digest 和 ID type separation 的 schema golden/property test。
- Built-in risk floor、每个 policy decision 与 approval transition 的 table test。
- Adversarial Shell corpus 覆盖 tab、newline、unspaced metacharacter、path substitution、symlink/mount
  change、environment injection 与 executable replacement。
- 覆盖 workspace、UID、boot/agent epoch、container 与 remote instance 的 target substitution test。
- Permit expiry、replay、tamper、stale fence、cross-actor/Task/target use 与 revoke 测试。
- Concurrent consume test 证明一个 permit 最多产生一个 claimed Execution ID。
- 在 claim、audit start、OS invocation、result capture 和 Task callback 前后执行 kill-point test。
- Typed success、typed failure、in-progress 与 unknown effect 的 reconciliation test。
- Bypass test 证明 enabled Gateway/Core/Shell/ACP/Skill/MCP mutation path 没有 `PermitVerifier` 就不能到达
  executor。

## 开放问题

| 问题 | Owner | Phase 1 默认值 |
| --- | --- | --- |
| Canonical local/remote target identity 是什么？ | Phase 0 identity/security | 只支持 local pinned identity；remote blocked。 |
| 跨进程 permit 使用 opaque 还是 signed？ | Broker/security | Local 使用 opaque ledger-backed handle；进程边界使用 integrity proof。 |
| 哪种 audit mode 可以授权 mutation？ | Security/audit | Permit issuance 与 execution start 必须 Required。 |
| 是否允许 opaque compound shell？ | Security/executor | Initial profile deny；优先 typed operator。 |
| Post-crash effect 如何 reconcile？ | Target owner | 每种 operation 使用 typed probe；unknown 则 suspend Task。 |
| 何时删除 legacy direct CLI？ | Product/release | Parity、recovery 和 bypass inventory 验收后。 |
