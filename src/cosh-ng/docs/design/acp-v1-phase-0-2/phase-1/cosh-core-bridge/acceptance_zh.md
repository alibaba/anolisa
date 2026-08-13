# Phase 1 Cosh Core Bridge 验收基线

[English](acceptance.md) | [设计](design_zh.md)

## 基线结果

**整体结果：基于上游 `e3763b001c91f3c13dc6afbd57aac924162e9f59` 的候选实现为 PARTIAL，
Phase 1 仍是 NOT ACCEPTED。** 候选实现加入 neutral `AgentRuntimePort`，并在严格的 private
cosh-core JSONL v1 codec 上实现 supervised `CoshCoreBridge`。Bridge 已约束 public identity 与 event
顺序、限制 retained state，并通过 process cleanup 结算 cancel。Durable coordination、brokered tool
execution、resume/recovery、Shell ownership migration 与 real-provider evidence 仍缺失。

## 结果口径

| 结果 | 含义 |
| --- | --- |
| PASS | 基线证据准确满足可复用或最终验收项。 |
| PARTIAL | 已实现并测试局部基础，但仍缺少集成或必要 failure evidence。 |
| FAIL | 当前行为违反目标 production invariant。 |
| NOT IMPLEMENTED | 所需 Gateway path 不存在。 |
| BLOCKED | 指定 prerequisite 决策阻止验证。 |

## 已检查证据

- 上游源码基线：`e3763b001c91f3c13dc6afbd57aac924162e9f59`。
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
  private v1 initialization 与 typed wire observation，不使用 ACP 命名。
- [`runtime/port.rs`](../../../../../crates/cosh-gateway/src/runtime/port.rs) 定义 provider-neutral、
  object-safe command/event boundary 与脱敏 error。
- [`runtime/cosh_core_bridge.rs`](../../../../../crates/cosh-gateway/src/runtime/cosh_core_bridge.rs) 绑定
  COSH identity、映射有界 public event、拒绝不支持的 control request，并在不导入 Task storage、core
  或 Shell crate 的前提下独占一个 supervisor generation。

## 验收矩阵

| ID | 验收项 | 基线 | 证据或缺失产物 |
| --- | --- | --- | --- |
| CCB-001 | Bridge 实现 neutral `AgentRuntimePort`。 | Library 切片 PASS | Object-safe port 与 Core implementation 可编译，并通过 focused lifecycle test。 |
| CCB-002 | Private JSONL v1 与 ACP v1 显式分离。 | PASS | 当前 runtime contract 明确该区分。 |
| CCB-003 | Task input admission 前 exact initialization 成功。 | PARTIAL | Bridge 在 Prompt 前完成 negotiation，并要求先交付 `SessionOpened`；尚未集成 durable Task admission。 |
| CCB-004 | Gateway production 拒绝 legacy unversioned peer。 | PARTIAL | Bridge 使用 strict codec，拒绝 missing/mismatched version；仍缺已安装 Core profile evidence。 |
| CCB-005 | `RuntimeSupervisor` 是 child process lifecycle 唯一 owner。 | PARTIAL | 新 supervisor 独占一个 child/group/pipe/reap；现有 Shell core owner 与 restart policy 尚未迁移。 |
| CCB-006 | 每种 JSONL message 映射成有界有序 Runtime event/command。 | PARTIAL | Session、text、tool observation、result、cancel 与 transport failure 已用 monotonic sequence 映射；question/auth/tool permission、usage、environment、durable backpressure 与完整 golden 仍缺。 |
| CCB-007 | Task/Run/runtime/Agent/provider ID 保持独立。 | PARTIAL | Bridge 创建带 scoped provider `ExternalRef` 的 fenced in-memory binding；仍缺 durable binding persistence 与 stale-generation reconciliation。 |
| CCB-008 | Bridge 不能写 Task storage。 | Library 切片 PASS | Dependency 与 source review 表明 port/bridge 没有 storage owner 或 storage call。 |
| CCB-009 | Brokered profile 阻止 core-local side effect。 | FAIL | 当前 allowed/approved tool 可在 core 执行。 |
| CCB-010 | `can_use_tool` 进入 Broker 和 permit-bound target result。 | NOT IMPLEMENTED | Broker/Bridge 不存在。 |
| CCB-011 | Approval receipt 在 durable Task ownership 后发送。 | NOT IMPLEMENTED | 当前 receipt 只证明 Shell main-thread receipt。 |
| CCB-012 | Question/auth/evidence 使用 durable 或 secret-safe port。 | PARTIAL | Core auth 与所有未建模 control request 使用脱敏 error fail closed；durable port 仍缺。 |
| CCB-013 | Process cancel escalation、kill group 并 reap child。 | PARTIAL | Focused test 覆盖 interrupt、cancelled terminal、TERM/KILL/reap 与同步 fallback cleanup；仍缺 descendant 与 cancel/result/EOF race fixture。 |
| CCB-014 | Provider session persistence 与 Task storage 分离。 | PASS | 当前 `SessionStore` 是 workspace-scoped provider state。 |
| CCB-015 | Crash/restart 不会静默重发 uncertain prompt。 | NOT IMPLEMENTED | Task/Broker reconciliation 不存在。 |
| CCB-016 | Gateway 不通过 Rust dependency 依赖 core implementation 或 Shell。 | PASS | `cosh-gateway` mirror private wire type，不依赖 core/Shell crate。 |
| CCB-017 | Brokered tool inventory 与 private-protocol extension 决策已固化。 | BLOCKED | Core/Broker owner 决策未完成。 |

当前 Shell behavior 的 PASS 只表示可复用 baseline evidence，不证明未来 Gateway-owned path 已存在。

## 要求的 fixture、命令与产物

| 产物 | 必须提供的证明 |
| --- | --- |
| `cosh-jsonl-v1` canonical corpus | 每种 input/output、optional capability、malformed 与 oversized case。 |
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
cargo test --package cosh-gateway cosh_jsonl_contract
cargo test --package cosh-gateway-contracts runtime_schema
```

当前未提交候选实现的 bridge-targeted evidence：

```bash
cargo +1.88.0 test --locked --package cosh-gateway cosh_core_bridge
# Library target 7 passed；0 failed；104 filtered out
```

这覆盖 identity fencing、stream 与 terminal mapping、single terminal delivery、open timeout、cross-Run
rejection、SessionOpened-before-Prompt ordering、idle-cancel rejection、aggregate Prompt bound、retained
tool-ID bound，以及 cancel 时的 process cleanup。它不能替代必需的 canonical corpus、完整
process-tree/race、Broker、recovery、backpressure、Shell protocol、real-provider 或 PTY gate。Rustdoc 与
Clippy evidence 只会在最终 scoped command 完成后记录。

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
