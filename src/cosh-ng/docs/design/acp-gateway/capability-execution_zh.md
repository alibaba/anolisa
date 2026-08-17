# Capability 与执行

[English](capability-execution.md)

关联架构：[COSH Gateway 与 ACP 架构](README_zh.md)

## 目的

本文把 Runtime request、用户 approval、COSH execution authority 与外部 effect 分开。
任何 protocol callback 或 UI action 本身都不是 Permit。

## Authority 模型

```text
Runtime request
  -> CapabilityRequest
  -> Policy decision
  -> Approval, when required
  -> ExecutionPermit
  -> Execution claim
  -> durable pre-effect audit
  -> typed ExecutionTarget
  -> typed result or uncertainty
  -> Runtime acknowledgement/result delivery
```

权威对象包括：

| 对象 | 语义 |
| --- | --- |
| `CapabilityRequest` | Canonical operation、actor、target、scope 与 digest |
| `PolicyDecision` | 有版本的 allow、deny 或 require-approval 结果 |
| `Approval` | 绑定 request 与 expiry 的持久人工决策 |
| `ExecutionPermit` | 绑定准确 request、target、policy 与 fence 的单次权限 |
| `Execution` | 一次外部 effect 的 claim/start/complete 生命周期 |
| `ExecutionTarget` | 校验并执行一个 typed operation 的可信 adapter |

Provider-native permission 只有在 COSH 通过上述流程接管 operation 后才是治理权限，否则
只是 observation evidence。ACP `allow_once` 不能证明 COSH Permit 在 effect boundary 被消费。

## ExecutionTarget registry 与 profile boundary

Gateway 负责公共 approval、Permit、lease、dispatch 与 recovery 状态机。`ExecutionTarget`
provider 只负责自身 typed operation mapping、readiness proof、target identity、effect、result
与 reconciliation logic。

Provider 由可信 daemon construction 在 admission 前注册。Capability profile 从 registry 中
选择封闭的 operation-version 与 target-kind 集合。Task input、Runtime label 与 approval
presentation 都不能加载 provider 或改变该集合。

`task-only-v1` 选择空 governed-operation 集合，因此不依赖 checkpoint provider。
`ws-ckpt-v1` 选择 typed workspace checkpoint operation，并要求 `ws-ckpt` provider 在
Gateway 接收 Task 前通过 readiness 与 identity admission。后续 Git、OverlayFS、cloud 或
remote snapshot provider 必须拥有独立 typed target 与 conformance evidence，不能作为
`ws-ckpt` 的 alias 或 silent fallback。

Provider 缺失在 profile admission 阶段处理。已经 admitted 的 Runtime 若请求 profile 外的
operation，Gateway 必须在创建 Approval 或 Permit state 前返回稳定 unsupported error。

## Canonical operation

每个 governed operation 都有有版本的 typed representation。Canonical digest 覆盖：

- operation name 与 version；
- 有界 input；
- Actor、Task、Run 与 Request identity；
- target identity 与 scope；
- policy revision 与 expiry；
- Runtime binding 与 lease generation。

Presentation text 不是 authority。Runtime label 可以帮助用户理解，但不能改变 operation
digest、target 或 policy 语义。

## Approval

- Approval 是 asynchronous 且 durable 的。
- 只有经过认证并对 Task 有权限的 actor 可以 resolve。
- Approve 与 deny 通过同一个带 revision 的状态机竞争。
- Expiry 是 terminal denial source，不得创建 allow dispatch。
- Idempotent replay 返回原 durable receipt。
- 第一次签发 Permit 前重新 evaluate policy 与 target。
- 已经持久签发的 Permit 可以恢复，不重新发明 policy decision。

## Permit 与 fence

Permit 具有以下性质：

- single-use；
- 绑定一个 `ExecutionId` 与 typed operation；
- 绑定 target identity digest；
- 绑定 RuntimeBinding 与 Run lease generation；
- 受 expiry 和 policy revision 约束；
- execution claim 时原子消费。

同一 generation 内的 lease renewal 不使 authority 失效。Takeover generation 会 fence 所有
尚未安全消费的旧 Permit。

## Effect 前 audit

准确 Execution start 的 security audit record durable 之前不得开始执行。Audit boundary 保存
request、policy、approval、Permit、execution、target 与 fence 的有界 reference，不保存
raw secret 或无限 tool input。

Target 调用前 audit persistence 失败时，execution 收敛为 known no effect。Audit durability
本身不确定时也不得开始 target execution。

## Execution 生命周期

```text
Planned -> Claimed -> Started -> Succeeded | Failed | Uncertain
```

- `Claimed` 证明 Permit 已消费，但 target 尚未启动。
- `Started` 证明 audit barrier 已通过，effect 可能已经发生。
- `Succeeded` 持久化 typed result。
- `Failed` 只有在 target 能证明时才是 conclusive。
- `Uncertain` 表示 effect 可能发生，不得自动 retry。

Typed result 与 terminal Execution state 原子提交。Runtime delivery 使用独立 durable dispatch
ledger，使响应丢失不会重复 effect。

## Reconciliation

`ExecutionTarget` 可以提供 query-only reconcile。输入为持久化的准确 operation、target
identity 与 execution reference。

- Exact match 可以收敛为 `Succeeded` 或 conclusive `Failed`。
- 只有 target 能证明 effect 未发生时，absence 才是 conclusive。
- Identity changed、evidence incomplete、timeout 或 query failure 继续保持 `Uncertain`。
- Reconciliation 不得重复 mutation。

## Cancellation

Cancellation 阻止尚未开始的 authority 继续推进，并收敛 pending approval/input。它不能擦除
已经到达 `Started` 的 Execution。取消 uncertain execution 后，Task 继续以 suspended 形式可见，
直到 reconcile 或 operator settlement。

## 验收不变量

- 未消费准确 Permit 且无 durable start audit 时不得执行。
- Denial、expiry、stale fence 与 policy change 的 target call 数为零。
- Permit 跨进程重启也只能 claim 一次。
- Started execution 不得自动 retry。
- 伪造 Runtime result 不能与 durable Execution state 不一致。
- Delivery replay 最多写 Runtime 一次。
- Profile 不得暴露没有 exact admitted target provider 的 operation。
- Profile 移除 `ws-ckpt` 时必须同时移除 checkpoint tool 与 authority，不能把 checkpoint
  request 转发到其他 execution path。
