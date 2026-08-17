# Phase 1 Cosh Core Bridge 验收基线

[English](acceptance.md) | [设计](design_zh.md)

## 基线结果

**整体结果：基于上游 `e90d9d94` 的候选实现为 PARTIAL，Phase 1 仍是 NOT ACCEPTED。**
候选实现加入 neutral `AgentRuntimePort`，并在两个明确私有的 COSH profile 上实现 supervised
`CoshCoreBridge`：legacy Shell/Core v1 与 Gateway brokered v2。Bridge 已约束 public identity 与
event 顺序、限制 retained state，并通过 process cleanup 结算 cancel。受约束的 Gateway production
profile 是 task-only，只有 `ask_user_question`，没有 production `ExecutionTarget`，也不依赖
checkpoint/ws-ckpt。通用 Capability/Permit/Execution contract 属后续基础。更广的 tool execution、
resume/recovery、Shell ownership migration、real-provider evidence 与人工 Terminal 验证仍未验收。

## 结果口径

| 结果 | 含义 |
| --- | --- |
| PASS | 基线证据准确满足可复用或最终验收项。 |
| PARTIAL | 已实现并测试局部基础，但仍缺少集成或必要 failure evidence。 |
| FAIL | 当前行为违反目标 production invariant。 |
| NOT IMPLEMENTED | 所需 Gateway path 不存在。 |
| BLOCKED | 指定 prerequisite 决策阻止验证。 |

## 已检查证据

- 上游源码基线：`e90d9d94`。
- [`protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs) 定义 exact private protocol v1 和全部当前
  message shape。
- [`headless.rs`](../../../../../crates/cosh-core/src/headless.rs) negotiation 并运行 provider turn。
- [`session.rs`](../../../../../crates/cosh-core/src/session.rs) 和
  [`session/store.rs`](../../../../../crates/cosh-core/src/session/store.rs) 持久化 provider conversation。
- [`cosh_core_service.rs`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service.rs) 拥有当前 Shell
  persistent process 与 cancellation lifecycle。
- [`control_protocol.rs`](../../../../../crates/cosh-shell/src/adapter/control_protocol.rs) 在 standalone Shell
  内 mirror parser/serializer behavior。
- [`runtime/supervisor.rs`](../../../../../crates/cosh-gateway/src/runtime/supervisor.rs) 独占一个 child
  process group、有界 pipe、TERM/KILL escalation、reap 与 process terminal delivery。
- [`runtime/bounded_io.rs`](../../../../../crates/cosh-gateway/src/runtime/bounded_io.rs) 实现 bounded
  stdout framing 与 stderr-tail retention。
- [`runtime/cosh_core_jsonl.rs`](../../../../../crates/cosh-gateway/src/runtime/cosh_core_jsonl.rs) 实现严格的
  private v1/v2 initialization 与 typed wire observation，不使用 ACP 命名。
- [`runtime/port.rs`](../../../../../crates/cosh-gateway/src/runtime/port.rs) 定义 provider-neutral、
  object-safe command/event boundary 与脱敏 error。
- [`runtime/cosh_core_bridge.rs`](../../../../../crates/cosh-gateway/src/runtime/cosh_core_bridge.rs) 绑定
  COSH identity、映射有界 public event、拒绝不支持的 control request，并在不导入 Task storage、core
  或 Shell crate 的前提下独占一个 supervisor generation。

## 验收矩阵

| ID | 验收项 | 基线 | 证据或缺失产物 |
| --- | --- | --- | --- |
| CCB-001 | Bridge 实现 neutral `AgentRuntimePort`。 | Library 切片 PASS | Object-safe port 与 Core implementation 可编译，并通过 focused lifecycle test。 |
| CCB-002 | Private COSH v1/v2 与 ACP v1 显式分离。 | PASS | Shared dual-version corpus 与两侧 codec 都使用 COSH 名称和版本，两个 profile 均不冒充 ACP。 |
| CCB-003 | Task input admission 前 exact initialization 成功。 | PARTIAL | Gateway 在 Prompt 前完成 brokered v2 negotiation 并要求先交付 `SessionOpened`；daemon 在 prompt 前持久化 Runtime binding，完整 Core recovery 仍缺。 |
| CCB-004 | Gateway production 拒绝 legacy、missing 与 mismatched negotiation。 | PASS | Cross-implementation negative fixture 在 input 前拒绝错误或缺失的 version/profile/capability；production 要求精确 brokered v2。 |
| CCB-005 | `RuntimeSupervisor` 是 child process lifecycle 唯一 owner。 | PARTIAL | 新 supervisor 独占一个 child/group/pipe/reap；现有 Shell core owner 与 restart policy 尚未迁移。 |
| CCB-006 | 每种 JSONL message 映射成有界有序 Runtime event/command。 | PARTIAL | Session、text、tool observation、result、cancel 与 transport failure 已用 monotonic sequence 映射；question/auth/tool permission、usage、environment、durable backpressure 与完整 golden 仍缺。 |
| CCB-007 | Task/Run/runtime/Agent/provider ID 保持独立。 | PARTIAL | Daemon 持久化 fenced Runtime binding 并拒绝 stale generation；完整 provider-session recovery 仍缺。 |
| CCB-008 | Bridge 不能写 Task storage。 | Library 切片 PASS | Dependency 与 source review 表明 port/bridge 没有 storage owner 或 storage call。 |
| CCB-009 | 启用的 Gateway brokered profile 阻止 core-local side effect。 | Task-only scope PASS | Immutable inventory 只有无 side effect 的 `ask_user_question`；extension、Skill、MCP、hook、Shell、file、process、network 与 checkpoint path 均不存在或禁用。这不是通用 Broker 声明。 |
| CCB-010 | 该 profile 启用的每个 side effect 都进入 Broker 和 permit-bound typed result。 | NOT IMPLEMENTED | Task-only profile 没有 production side-effect operation 或 `ExecutionTarget`。 |
| CCB-011 | Approval receipt 在 durable Task ownership 后发送。 | PARTIAL | Task-owned question/input state 已持久；approval-to-permit 与 execution-result dispatch 属后续 capability。 |
| CCB-012 | Question/auth/evidence 使用 durable 或 secret-safe port。 | PARTIAL | Runtime v4 input request 已进入 durable exact-pending Task state；private typed dispatch row 保存 raw response，Task event 与 receipt 只保存 digest。Core auth/evidence 与更广 question mapping 仍 fail closed 或缺失。 |
| CCB-013 | Process cancel escalation、kill group 并 reap child。 | PARTIAL | Focused test 覆盖 interrupt、cancelled terminal、TERM/KILL/reap 与同步 fallback cleanup；仍缺 descendant 与 cancel/result/EOF race fixture。 |
| CCB-014 | Provider session persistence 与 Task storage 分离。 | PASS | 当前 `SessionStore` 是 workspace-scoped provider state。 |
| CCB-015 | Crash/restart 不会静默重发 uncertain prompt。 | PARTIAL | Runtime binding/restart convergence 已持久且 fail closed；side-effect uncertainty reconciliation 与完整 prompt/recovery fixture 属后续工作。 |
| CCB-016 | Gateway 不通过 Rust dependency 依赖 core implementation 或 Shell。 | PASS | `cosh-gateway` mirror private wire type，不依赖 core/Shell crate。 |
| CCB-017 | Phase 1 brokered inventory 与 private-protocol profile 决策已固化。 | Scope decision PASS | Gateway production 使用 private COSH v2，只暴露 task-only `ask_user_question`。Legacy v1 留给 standalone Shell；checkpoint/ws-ckpt 与 Shell attachment/owner migration 属后续工作。 |

Legacy Shell behavior 与 ACP `doctor`/`run` interoperability 是 ungoverned compatibility path，不证明
Gateway-governed path 已存在。

## 要求的 fixture、命令与产物

| 产物 | 必须提供的证明 |
| --- | --- |
| `cosh-private-wire-dual-version` canonical corpus | Legacy v1 initialize/ack、brokered v2 task/question request/ack/result，以及 wrong/missing version/profile/capability case。 |
| Cross-implementation fixture report | Core encoder、Shell mirror 与 Gateway decoder 一致。 |
| `runtime-supervisor-killpoints` | Spawn、negotiate、stream、cancel、EOF、wait、shutdown 与 restart race。 |
| `runtime-event-mapping` golden | 每种 message 的有界 normalized event 与 ID correlation。 |
| `brokered-tool-inventory` | 每个 exposed side-effecting tool 都 delegated 或 disabled。 |
| Provider-session recovery matrix | New、resume、mismatch、corrupt、stale、cancel 与 restart。 |
| Backpressure fixture | Durable sink outage 不会丢 control 或 terminal event。 |

实现后预期执行：

```bash
cargo test --package cosh-gateway cosh_core_bridge
cargo test --package cosh-gateway runtime_supervisor
cargo test --package cosh-gateway cosh_core_jsonl
cargo test --package cosh-core --test jsonl_protocol
cargo test --package cosh-gateway-contracts runtime_schema
```

当前未提交候选实现的 bridge-targeted evidence：

```bash
cargo +1.88.0 test --locked --package cosh-gateway cosh_core_bridge
cargo +1.88.0 test --locked --package cosh-core --test jsonl_protocol
cargo +1.88.0 test --locked --package cosh-gateway cosh_core_jsonl
```

这覆盖 identity fencing、stream 与 terminal mapping、single terminal delivery、open timeout、cross-Run
rejection、SessionOpened-before-Prompt ordering、idle-cancel rejection、aggregate Prompt bound、retained
tool-ID bound、cancel 时的 process cleanup，以及 shared dual-version wire corpus。它不能替代完整
process-tree/race、通用 Broker、recovery、backpressure、real-provider 或 PTY gate。Rustdoc 与 Clippy
evidence 只会在最终 scoped command 完成后记录。

## Exit criteria

1. CCB-001 至 CCB-016 全部 PASS，且 CCB-017 有 accepted profile/version decision。
2. Canonical fixture、mapping、process-race、session-recovery、Broker bypass 与 backpressure suite 在 exact
   candidate commit 上通过并记录 count。
3. Dependency check 证明 Gateway 不 link core implementation 或 standalone Shell，并且 Bridge/
   RuntimeSupervisor 不能写 Task storage，或绕过 Broker 执行 OS 工作。
4. Security review 覆盖 executable/workspace pinning、environment allowlist、protocol parser limit、
   correlation、secret/auth flow、provider session scope、approval receipt timing、cancel 与 uncertain execution。
5. 报告记录 executable/profile configuration、private protocol version、exact command、fixture、unsupported
   tool、restart policy、untested real-provider path 与 rollback。

## 当前风险

- 复用 Shell `AgentAdapter` type 会引入 presentation 与 CommandBlock coupling。
- 把 private JSONL 称作“ACP”会产生虚假 interoperability 与 version assumption。
- 对 side-effect tool 发送 generic allow 会绕过 target-bound permit。
- 从 stale Run 持久化 provider session binding，可能使后续工作关联到错误 Task。
- 读取速度超过 durable Task event commit，可能在 daemon crash 时丢失 control event。
- `ExternalRef.value` 包含私有 provider data，不得写入 log 或通用 audit output；durable storage 仍需采用
  encrypted reference row 或 keyed digest policy。
