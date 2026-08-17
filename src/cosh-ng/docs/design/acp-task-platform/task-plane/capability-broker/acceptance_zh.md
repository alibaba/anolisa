# Phase 1 Capability Broker 验收报告

[English](acceptance.md) | [设计](design_zh.md)

## 结果

**整体结果为 PARTIAL。通用 Broker foundation 已存在，Phase 1 尚未通过。**
实现 worktree 基于 `e90d9d9402c7fa1c8122267eb4e075c0adda51f5`。

基础 Broker logic 会在 policy 前校验 Capability request expiry，以及 authoritative Task、Run、完整 Actor
provenance、target、operation descriptor、完整 operation digest 与 requested scope。它将 policy
decision 与 permit 分开，为 permit 绑定准确 authority，并通过 process-local memory store atomically
consume single-use permit。八个 targeted test 通过。这些是通用 contract/logic foundation，不是
production execution 证据。

该结果不构成通用治理声明。Production `CoshBrokered` profile 是 task-only，只有
`ask_user_question`，没有 production `ExecutionTarget`，也不依赖 checkpoint/ws-ckpt。通用
Capability、Permit 与 Execution contract/ledger row 作为后续基础保留。Shell、Skill、MCP、扩展工具、
legacy CLI 与 interactive Core mutation path 仍在该边界之外。

Owner scope decision 如下：下表 Phase 1 criterion 只适用于 enabled Gateway production profile。
Production `serve` 与 library daemon 只接纳 `core` / `gateway-brokered-v1`。ACP `doctor`/`run`
interoperability、standalone Shell 与 legacy CLI 明确 ungoverned；这些 path 的 compatibility evidence
不能满足 Broker criterion。Formal universal Broker exit 仍未满足。

## Durable ledger storage 结果

**整体结果为 VERIFIED DURABLE TASK STORAGE SLICE；Production Broker integration 为 NOT IMPLEMENTED。**
Checksummed SQLite schema v9 现在会持久化 Task/Run event 与 projection、approval/input state、Outbox intent、
Runtime binding、fenced Run lease 与 durable dispatch receipt。通用 permit/execution ledger contract 作为
后续基础保留；本 PR 不接入 production target，也不声称 checkpoint execution、typed side-effect result
或 reconciliation。

Durable ledger suite 使用
`cargo test --locked --package cosh-gateway storage --no-fail-fast` 复现。其 fixture 覆盖 stale lease、
cross-Task Run、generation skip、扩大
approval deadline、integer overflow、idempotency namespace、receipt corruption 与 atomic rollback。

本 PR 没有 checkpoint driver、ws-ckpt resolver、production target 或 pre-effect execution loop。
Target resolution、durable permit issuance、audit gate、execution 与 reconciliation 属后续可选
capability，仍待完成。

## 结果口径

| 结果 | 含义 |
| --- | --- |
| PASS | 可复现证据满足所述 scope 的完整验收项。 |
| PARTIAL | 实现证据只满足明确列出的子集。 |
| FAIL | 当前启用 path 违反目标 invariant。 |
| NOT IMPLEMENTED | 该验收项没有实现。 |
| BLOCKED | 前置决策阻止验证。 |

## 实现证据

| 来源 | 已验证行为 |
| --- | --- |
| [`capability.rs`](../../../../../crates/cosh-gateway/src/capability.rs) | 公开 Broker、policy、permit-store、claim、context 与 memory-store 边界，不公开 executor |
| [`broker.rs`](../../../../../crates/cosh-gateway/src/capability/broker.rs) | 在 policy 前校验 expiry、Task/Run/完整 `ActorRef` 与 authoritative target/descriptor/完整 operation digest/scope；拒绝 unavailable 或 invalid policy authority，并公开 atomic claim |
| [`memory.rs`](../../../../../crates/cosh-gateway/src/capability/memory.rs) | 在同一个 mutex 内校验并 consume permit；mismatch、expiry 与 replay fail closed |
| [`memory/tests.rs`](../../../../../crates/cosh-gateway/src/capability/memory/tests.rs) | 覆盖 parent 与 actor-provenance substitution、policy branch/failure、permit binding、mismatch、expiry/replay 与 concurrent consumption |
| [`capability.rs`](../../../../../crates/cosh-gateway-contracts/src/capability.rs) | 定义中立 request、decision、approval 与 permit contract，包含 Actor/Task/Run/Execution/target/operation/policy/expiry binding |
| [`scheduler.rs`](../../../../../crates/cosh-gateway/src/daemon/scheduler.rs) | 保持 durable Task/Run/Outbox/lease/input/cancel/retry/recovery coordination 与未来 execution-target adapter 分离 |

Broker source 只依赖 contracts 和两个显式 port，不 import Task storage、Runtime bridge、OS operator、
ACP 或 network API。

## 验收矩阵

| ID | 验收项 | 结果 | 证据或剩余缺口 |
| --- | --- | --- | --- |
| CBR-001 | Gateway production profile 启用的每个 side effect 使用 typed `CapabilityRequest`。 | Task-only scope PASS | Immutable inventory 只有无 side effect 的 `ask_user_question`；production 没有 enabled side effect。 |
| CBR-002 | 其 target 解析成 immutable authenticated local identity。 | NOT IMPLEMENTED | 没有 production `ExecutionTarget` 或 target resolver；checkpoint/ws-ckpt identity 属后续工作。 |
| CBR-003 | 该 path 的 policy result、approval 与 permit 是不同类型。 | PARTIAL | 通用 contract 与 in-memory logic 区分这些类型；production 没有 side-effect path 签发它们。 |
| CBR-004 | 该 profile 每个 permitted effect 有一个 `ExecutionId`。 | NOT IMPLEMENTED | Task-only inventory 没有 permitted OS effect 或 production execution target。 |
| CBR-005 | 其 permit 绑定 actor、Task、Run、target、operation digest、policy、fence、expiry 与一次使用。 | PARTIAL | 通用 permit validation 在 logic test 中覆盖 binding；durable production issuance 属后续工作。 |
| CBR-006 | 其 target 在执行前立即校验并 consume permit。 | NOT IMPLEMENTED | 没有 production target，因此不声称 target-side consume 或 execution loop。 |
| CBR-007 | 其 approval 是 durable Task state 且不能扩大 authority。 | PARTIAL | Durable Task approval state 已有；approval-to-permit 与 target binding 属后续工作。 |
| CBR-008 | Broker 不写 Task aggregate。 | PASS | Broker 不依赖 Task aggregate 或 storage，只返回 decision。 |
| CBR-009 | 该 profile 重复 execute 不能产生第二个 effect。 | NOT IMPLEMENTED | Task-only profile 没有 enabled production effect 或 execution target。 |
| CBR-010 | Crash uncertainty 进入 typed reconciliation，不自动 retry。 | PARTIAL | Task/Run restart recovery 与 fail-closed retry boundary 已有；typed side-effect reconciliation 属后续工作。 |
| CBR-011 | Governed profile 不启用 opaque Shell fallback。 | Task-only scope PASS | Accepted inventory 不含 Shell 或其他 side-effect operation；legacy parser behavior 只算 compatibility evidence。 |
| CBR-012 | Typed policy 有 allow/deny/require-approval outcome。 | PASS | Neutral `PolicyPort` 与 deterministic test 覆盖三个 outcome，以及 unavailable/invalid authority。 |
| CBR-013 | 该 profile 的 execution start 要求 durable security audit。 | NOT IMPLEMENTED | Task-only profile 没有 production execution start 或 target。 |
| CBR-014 | 该 profile 禁用或 delegated Core direct side-effecting tool。 | Task-only scope PASS | Immutable inventory 只有 `ask_user_question`；hook、MCP、Skill、扩展、Shell、file、process、network 与 checkpoint path 均禁用。 |
| CBR-015 | Governed mode 下 production Gateway operation 不能绕过 permit。 | Task-only scope PASS | `serve` 与 library daemon 只接纳 task-only selector；没有 production side-effect operation 可以绕过 permit。ACP `doctor`/`run` 与 legacy CLI 明确不在 governed claim 内。 |
| CBR-016 | Remote identity 在 v2 attestation 决策批准前保持禁用。 | Scope decision PASS | Phase 1 是 local installation-scoped single-tenant；remote 与 `TenantId`/multi-tenant support 属未来 v2。 |

## 验证证据

从 `src/cosh-ng` 运行：

```text
cargo fmt --package cosh-gateway -- --check
cargo test --locked --package cosh-gateway-contracts
result: package test 与 doc-test target passed

cargo test --locked --package cosh-gateway capability::
result: targeted capability suite passed

cargo clippy --locked --package cosh-gateway-contracts --all-targets -- -D warnings
cargo clippy --locked --package cosh-gateway --all-targets -- -D warnings
result: passed with zero warnings

cargo doc --locked --package cosh-gateway-contracts --package cosh-gateway --no-deps
result: passed
```

八个测试证明：

- Request expiry，以及 Task、Run、Actor ID、issuer、assurance、target、operation descriptor、
  完整 operation digest 与 scope substitution 在 policy 前 fail closed；
- Policy deny 与 approval 不会创建 permit；
- Policy unavailable、revision 为零与 authority 过期 fail closed；
- Issued permit 绑定 actor、Task、Run、Execution、target、完整 canonical operation digest、policy revision、expiry
  与一次使用；
- 错误 actor、Task、Run、Execution、target、完整 operation digest 或 policy revision 不 consume authority；
- Expired 与重复 consume fail closed；
- 八个同时 claim 只有一个成功。

通用 contract/ledger 与 scheduler suite 已执行。不声称 checkpoint adapter/driver、ws-ckpt resolver、
production target 或 side-effect execution loop。Destructive package-unit containment fixture 已在
disposable Ubuntu 24.04 arm64/systemd 255 上通过。
不声称已验收真实 Codex/Claude、人工 Terminal、ECS、network 或通用工具验证。

## 必要的剩余产物

| 产物 | 必须提供的证明 |
| --- | --- |
| 通用 approval 与 re-authorization test | 每个 presentation path 都只能从 committed matching approval 签发 authority。 |
| Immutable target-substitution matrix | Workspace、UID、boot、container 与 instance change 使 permit 失效。 |
| Multi-operation permit/execution matrix | Generic invariant 对每个未来 governed operation 都成立。 |
| 通用 security audit gate | 每个 target 的 required audit persistence 失败时 issuance 与 execution start 都失败。 |
| Execution kill-point 与 reconciliation matrix | Claimed、started 与 uncertain effect 绝不自动 replay。 |
| Broker bypass inventory | 每个 enabled Gateway/Core/Shell/ACP/Skill/MCP effect 都到达 verifier。 |
| Revocation 与 lease-fence corpus | Revoked、stale-runtime 与 stale-policy authority fail closed。 |
| Trusted canonicalizer test | Independent canonicalization 在 Broker admission 前绑定 descriptor 与 digest。 |

## Exit Criteria

Task-only production profile 只满足有界 inventory 决策，Phase 1 仍为 PARTIAL，因为 production target
execution 尚未实现，已记录的 real-provider/manual release gate 也仍开放。Formal universal Broker exit 还要求：

1. 每个未来 enabled operation 的 approval resolution、permit issuance、durable consumption、audit、
   execution 与 typed reconciliation 形成一个经过评审的 security boundary。
2. 通用 immutable target identity、revocation 与 remote attestation 决策。
3. Bypass inventory 覆盖每个未来 enabled Gateway/Core/Shell/ACP/Skill/MCP mutation edge。
4. Crash、replay、substitution、audit-failure、revocation、real-provider 与 manual fixture 在同一个准确
   release candidate 上通过。

## 剩余风险

- `MemoryPermitStore` 仍只用于测试；任何 future target path 的 production authority 尚未接入。
- Target identity 与 execution coverage（包括 checkpoint/ws-ckpt）属于后续可选 capability，不是当前
  production evidence。
- Ungoverned Shell、ACP interoperability、Skill、MCP、扩展与 legacy effect 仍在封闭 production
  profile 之外，绝不能宣传为 governed。
