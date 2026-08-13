# Phase 1 Task Execution Plane 验收基线

[English](acceptance.md) | [设计](design_zh.md)

## 基线结果

**整体结果：`6c115aefe04ace0d169a24fa7cd55ad7c1befa52` 上为 NOT IMPLEMENTED。** 仓库已有
可靠的 provider-session persistence 与 audit evidence，但二者都不是持久 Task aggregate。当前不存在
coordinator、Task event store、Run lease、idempotency ledger 或 outbox。

本文是 readiness report，不是 Phase 1 行为已经通过的证据。

## 首个实现结果

**整体结果：首个切片已验证；Phase 1 Exit 尚未接受。** 当前工作树候选新增共享 Task ID/event、确定性
reducer 与 atomic SQLite Task store。它还没有实现唯一 writer `TaskCoordinator`、Run lease/fencing、
Outbox delivery worker 或 execution reconciliation。

2026-08-13 记录的证据：

- `cargo test --locked --package cosh-gateway task::aggregate --no-fail-fast` 通过 6/6 tests。
- `cargo test --locked --package cosh-gateway storage --no-fail-fast` 通过 14/14 tests。
- `cargo clippy --locked --package cosh-gateway --lib -- -D warnings` 通过。
- Test 覆盖 revision gap 错误不修改 aggregate、显式 approval waiting、deny 后 suspension、Run 与 Task
  terminal closure、in-memory schema-version rejection、actor substitution、actor-scoped idempotency
  replay/conflict、stale revision、Outbox atomic rollback、schema/checksum rejection、private-path attack、
  causation persistence，以及 durable reopen 后 event replay。

## 结果口径

| 结果 | 含义 |
| --- | --- |
| PASS | 固定源码和可复现产物满足该验收项。 |
| FAIL | 已存在的实现违反该验收项。 |
| PARTIAL | 已有 production 切片，但指定证据或行为仍不完整。 |
| NOT IMPLEMENTED | 该验收项没有 production path。 |
| BLOCKED | 指定上游决策或依赖阻止验证。 |

## 基线证据

- `git rev-parse HEAD` 确认为
  `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`。
- [`session.rs`](../../../../../crates/cosh-core/src/session.rs) 定义 provider-session schema、identity、
  generation、summary 与 health。
- [`session/store.rs`](../../../../../crates/cosh-core/src/session/store.rs) 使用 optimistic generation
  原子持久化一个 provider session。
- [`runtime/state.rs`](../../../../../crates/cosh-shell/src/runtime/state.rs) 属于 Shell in-memory
  presentation/runtime state。
- [`audit/event.rs`](../../../../../crates/cosh-types/src/audit/event.rs) 是 security evidence，不拥有 Task
  transition。
- 仓库搜索没有发现 `TaskCoordinator`、`TaskEventStore`、`TaskId` 或 Task outbox。

## 验收矩阵

| ID | 验收项 | 基线 | 证据或缺失产物 |
| --- | --- | --- | --- |
| TEP-001 | 存在 typed `TaskId`、`RunId` 和 lifecycle schema。 | PASS | `cosh-gateway-contracts::{ids,task}`。 |
| TEP-002 | Coordinator 是 aggregate 唯一 writer。 | NOT IMPLEMENTED | Coordinator 不存在。 |
| TEP-003 | State reducer 拒绝所有非法 transition。 | PARTIAL | Reducer 与关键 transition test 已通过；完整 state/event matrix 待补。 |
| TEP-004 | Event、snapshot、idempotency receipt 与 outbox 原子 commit。 | PASS | `commit_task` 使用 `BEGIN IMMEDIATE`；重复 Delivery ID 证明完整 rollback。 |
| TEP-005 | Expected revision 阻止 stale writer。 | PASS | Revision-conflict test 后所有 Task table 仍为空。 |
| TEP-006 | Run lease 使用 monotonic fencing 与有界 renewal。 | NOT IMPLEMENTED | Run lease 不存在。 |
| TEP-007 | Lease expiry 不会自动重放 unknown OS effect。 | NOT IMPLEMENTED | Reconciliation path 不存在。 |
| TEP-008 | Approval resolution 使用 first-valid-terminal-wins。 | PARTIAL | Reducer 拒绝已 resolve/非 pending ID；authorization 与 concurrent-decision fixture 待补。 |
| TEP-009 | Runtime 与 execution callback 幂等且带 fence。 | NOT IMPLEMENTED | Port 不存在。 |
| TEP-010 | Event replay 重建等价 projection。 | PASS | Durable reopen recovery replay ordered envelope，并比较完整 snapshot。 |
| TEP-011 | Outbox restart 使用 at-least-once 与稳定 Delivery ID。 | PARTIAL | 稳定 row 已持久化，但 dispatch lease/reclaim/ack 不存在。 |
| TEP-012 | Task record 排除 raw stream、secret 与 terminal buffer。 | PARTIAL | Snapshot/event leaf 已 typed/bounded；Outbox payload、collection aggregate bound 与 secret classification 待补。 |
| TEP-013 | Corrupt/incompatible history fail closed 且可 inspect。 | PARTIAL | Schema/replay 已 fail closed；quarantine 与 inspect surface 待补。 |
| TEP-014 | Provider `SessionStore` 与 Task storage 保持分离。 | PASS | Gateway SQLite 使用独立 crate/store 与 schema。 |
| TEP-015 | Final storage engine 与 durability profile 已批准。 | BLOCKED | Phase 0 storage ADR 未完成。 |

## 要求的 fixture、命令与产物

| 产物 | 必须提供的证明 |
| --- | --- |
| `task-events-v1` golden corpus | 稳定 codec、required/optional compatibility 与 bounds。 |
| 完整 transition table | 每个 state/command 组合都有 expected result。 |
| `task-store-vN` migration fixture | Upgrade、backup、inspect 与 incompatible-version 行为。 |
| Kill-point matrix | Commit 和 delivery ack 前、中、后的 atomicity。 |
| `expired-lease-uncertain-effect` | 新 worker suspend，而不是重新 execute。 |
| Concurrent approval fixture | 冲突 terminal decision 只有一个获胜。 |
| Replay digest artifact | Live projection 与 event-reduced projection 相同。 |

实现后预期运行：

```bash
cargo test --package cosh-gateway task_model
cargo test --package cosh-gateway task_store
cargo test --package cosh-gateway task_crash_recovery
cargo test --package cosh-gateway-contracts task_schema
```

当前实现 target 名比最初 placeholder 更宽。上文已经记录 exact targeted command 与 count。完整 workspace
gate 与 live/ECS validation 不属于本次按范围验证的首个切片。

## Exit criteria

1. TEP-001 至 TEP-014 全部 PASS，且 Phase 0 决策解除 TEP-015。
2. Model、concurrent-writer、crash、corruption、migration 与 reconciliation fixture 在 exact candidate
   commit 上通过。
3. Code-ownership check 证明 adapter、handler、bridge、worker 和 presenter 不能绕过 `TaskCoordinator`
   写 Task storage。
4. Security review 验证 tenant/workspace scope、actor/delegation、event redaction、lease fence、approval race、
   uncertain execution 与 store permission。
5. 验收报告列出 exact store engine/configuration、command、test count、artifact、unsupported migration path
   与 rollback procedure。

## 当前风险

- 扩展 provider `SessionStore` 会混淆 model conversation 与 control-plane truth。
- File-per-Task design 如果没有额外 transaction layer，可能无法原子 commit event/idempotency/outbox。
- 把 process PID 或 lease timeout 当成 completion，可能重复 side effect。
- 允许 presenter 或 callback 修改 approval state，会造成 split-brain authorization。
