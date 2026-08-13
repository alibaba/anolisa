# Phase 1 Cosh Core Bridge 验收基线

[English](acceptance.md) | [设计](design_zh.md)

## 基线结果

**整体结果：基于 `6c115aefe04ace0d169a24fa7cd55ad7c1befa52` 的工作树已有 PARTIAL
foundation。** 固定的上游基线仍是 NOT IMPLEMENTED。当前规划分支加入 Gateway-owned local process
supervisor 与严格的 private cosh-core JSONL v1 codec，但仍不存在已集成的 `CoshCoreBridge`、durable
runtime binding、public event mapping、brokered execution profile 或 Shell ownership migration。

## 结果口径

| 结果 | 含义 |
| --- | --- |
| PASS | 基线证据准确满足可复用或最终验收项。 |
| PARTIAL | 已实现并测试局部基础，但仍缺少集成或必要 failure evidence。 |
| FAIL | 当前行为违反目标 production invariant。 |
| NOT IMPLEMENTED | 所需 Gateway path 不存在。 |
| BLOCKED | 指定 prerequisite 决策阻止验证。 |

## 已检查证据

- 固定源码：`6c115aefe04ace0d169a24fa7cd55ad7c1befa52`。
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

## 验收矩阵

| ID | 验收项 | 基线 | 证据或缺失产物 |
| --- | --- | --- | --- |
| CCB-001 | Bridge 实现 neutral `AgentRuntimePort`。 | NOT IMPLEMENTED | 无 Gateway/port。 |
| CCB-002 | Private JSONL v1 与 ACP v1 显式分离。 | PASS | 当前 runtime contract 明确该区分。 |
| CCB-003 | Task input admission 前 exact initialization 成功。 | PARTIAL | Codec 在 user frame 前要求 exact v1/correlation/capabilities；尚未集成 Task admission。 |
| CCB-004 | Gateway production 拒绝 legacy unversioned peer。 | PARTIAL | Codec 拒绝 missing/mismatched version；尚无 launch profile 调用。 |
| CCB-005 | `RuntimeSupervisor` 是 child process lifecycle 唯一 owner。 | PARTIAL | 新 supervisor 独占一个 child/group/pipe/reap；现有 Shell core owner 与 restart policy 尚未迁移。 |
| CCB-006 | 每种 JSONL message 映射成有界有序 Runtime event/command。 | PARTIAL | 当前 output 解码为 typed local observation；缺少 public contract mapping/order/backpressure。 |
| CCB-007 | Task/Run/runtime/Agent/provider ID 保持独立。 | PARTIAL | Contracts 拥有 neutral ID，codec 单独命名 provider session；尚无 binding mapper。 |
| CCB-008 | Bridge 不能写 Task storage。 | NOT IMPLEMENTED | Boundary 不存在。 |
| CCB-009 | Brokered profile 阻止 core-local side effect。 | FAIL | 当前 allowed/approved tool 可在 core 执行。 |
| CCB-010 | `can_use_tool` 进入 Broker 和 permit-bound target result。 | NOT IMPLEMENTED | Broker/Bridge 不存在。 |
| CCB-011 | Approval receipt 在 durable Task ownership 后发送。 | NOT IMPLEMENTED | 当前 receipt 只证明 Shell main-thread receipt。 |
| CCB-012 | Question/auth/evidence 使用 durable 或 secret-safe port。 | NOT IMPLEMENTED | 当前 path 属于 Shell。 |
| CCB-013 | Process cancel escalation、kill group 并 reap child。 | PARTIAL | Supervisor TERM/KILL/reap test 通过；仍缺 descendant、cancel/result/EOF race 与 protocol interrupt fixture。 |
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

第一轮增量的 targeted evidence：

```bash
cargo test -p cosh-gateway --lib runtime --no-fail-fast
# 19 passed; 0 failed; 17 filtered out
```

这覆盖 codec negotiation/terminal behavior、bound、launch validation、stderr retention、single terminal
delivery 与 TERM-to-KILL reap。它不能替代必需的 canonical、process-tree/race、public mapping、Broker、
recovery、backpressure、Shell protocol 或 PTY gate。
Process suite 还注入 process-group TERM failure，并证明 direct child 会在返回前被 kill、reap、settle，
同时保留一个仍可读取且不可重复交付的 terminal。
其中 18 个通过测试由 Runtime 拥有；`runtime` 名称过滤还会选中一个名称包含 runtime event 的 Task
aggregate test。

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
