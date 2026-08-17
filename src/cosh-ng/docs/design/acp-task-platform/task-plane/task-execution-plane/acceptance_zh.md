# Phase 1 Task Execution Plane 验收基线

[English](acceptance.md) | [设计](design_zh.md)

## 基线结果

**整体结果：`6c115aefe04ace0d169a24fa7cd55ad7c1befa52` 上为 NOT IMPLEMENTED。** 仓库已有
可靠的 provider-session persistence 与 audit evidence，但二者都不是持久 Task aggregate。当前不存在
coordinator、Task event store、Run lease、idempotency ledger 或 outbox。

本文是 readiness report，不是 Phase 1 行为已经通过的证据。

## 首个实现结果

**整体结果：可运行 durable slice；Phase 1 Exit 尚未接受。** 当前工作树候选新增共享 Task ID/event、
确定性 reducer、atomic SQLite Task store、唯一 writer `TaskCoordinator`、fenced Run lease、Outbox
scheduling、durable Runtime binding、approval resolution 与 execution settlement。通用 governed
execution 与完整 crash/kill-point evidence 仍开放。

当前工作树证据通过以下 scoped command 验证：

- `cargo test --locked --package cosh-gateway task::aggregate --no-fail-fast`。
- `cargo test --locked --package cosh-gateway storage --no-fail-fast`。
- `cargo clippy --locked --package cosh-gateway --lib -- -D warnings` 通过。
- Test 覆盖 revision gap 错误不修改 aggregate、显式 approval waiting、deny 后 suspension、Run 与 Task
  terminal closure、in-memory schema-version rejection、actor substitution、actor-scoped idempotency
  replay/conflict、stale revision、Outbox atomic rollback、schema/checksum rejection、private-path attack、
  causation persistence，以及 durable reopen 后 event replay。

## Durable ledger 切片

**整体结果为 VERIFIED STORAGE SLICE；Phase 1 exit 尚未验收。** 当前 candidate 使用 checksummed
schema v9，持久化 approval/input state、runtime binding、Run lease、Outbox dispatch，以及 typed
Runtime input request/dispatch。通用 permit/execution ledger contract 作为可复用 foundation 保留，
但本 PR 没有 production `ExecutionTarget`、checkpoint loop 或 ws-ckpt dependency。每次 Task/ledger
mutation 在使用 Task/Run binding 前都会 replay authoritative Task event stream。Runtime event
acceptance 必须携带准确、当前且未过期的 lease generation 和 revision。`task.retry` 只会在旧 Run 已
静止后创建新的 fenced Run。Active Run 只有收到准确匹配的 pending Runtime input request 才会进入
waiting，单次匹配 response 才会恢复 running；raw input response 只存在 private dispatch row，Task
event 与 receipt 只保留 digest。

Focused evidence 使用以下命令复现：

- `cargo test --locked --package cosh-gateway storage --no-fail-fast`。
- Adversarial fixture 覆盖其他 Task 的有效 Run、stale lease revision、跳号 Runtime generation、
  跨 plane idempotency key 复用、SQLite integer overflow、terminal receipt divergence，以及 rejected
  ledger mutation 的完整 rollback。
- Task command 与 ledger command receipt table 强制共享 actor-scoped idempotency namespace。
  早期 migration checksum 保持不变，existing v1 store 可升级至 v9。

Daemon 现在已接线 `TaskCoordinator`、Outbox lease/reclaim/ack、Runtime dispatch 与 task/input recovery。
没有 checkpoint executor、ws-ckpt target 或 production side-effect result loop。该实现不是面向 Shell、
Skill、MCP、扩展工具或 legacy mutation path 的通用 executor/reconciliation service。

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
| TEP-002 | Coordinator 是 aggregate 唯一 writer。 | PARTIAL | Daemon command 与 scheduler settlement 使用 `TaskCoordinator`；仍需针对未来 adapter 做最终 ownership audit。 |
| TEP-003 | State reducer 拒绝所有非法 transition。 | 当前 schema PASS | 21 个 event × 9 个 state 的 exhaustive reducer matrix 覆盖合法与非法 transition，包括准确 pending input 与 retry，并验证 rejection 不修改 aggregate。 |
| TEP-004 | Event、snapshot、idempotency receipt 与 outbox 原子 commit。 | PASS | `commit_task` 使用 `BEGIN IMMEDIATE`；重复 Delivery ID 证明完整 rollback。 |
| TEP-005 | Expected revision 阻止 stale writer。 | PASS | Revision-conflict test 后所有 Task table 仍为空。 |
| TEP-006 | Run lease 使用 monotonic fencing 与有界 renewal。 | PASS | Lease acquire/renew/release 校验准确 owner、revision、generation、active Task/Run 与 deadline；takeover 增加 generation。 |
| TEP-007 | Lease expiry 不会自动重放 unknown OS effect。 | PARTIAL | Task/Run recovery fail closed 且不会自动 retry 未证明的 effect；typed side-effect reconciliation 属后续可选 capability。 |
| TEP-008 | Approval resolution 使用 first-valid-terminal-wins。 | PARTIAL | Durable pending-state CAS 与异步 API resolution 已有；approval-to-permit 与 production target integration 属后续工作。 |
| TEP-009 | Runtime 与 execution callback 幂等且带 fence。 | PARTIAL | Current lease/generation/sequence fence、durable dispatch receipt 与 source validation 覆盖 Task/Runtime callback；production execution-result replay 属后续工作。 |
| TEP-010 | Event replay 重建等价 projection。 | PASS | Durable reopen recovery replay ordered envelope，并比较完整 snapshot。 |
| TEP-011 | Outbox restart 使用 at-least-once 与稳定 Delivery ID。 | PARTIAL | Task-only slice 保留 dispatch leasing、reclaim 与稳定 Delivery ID；不声称 checkpoint-specific acknowledgement/result loop。通用 delivery、remote path 与 power-loss evidence 不在此结果内。 |
| TEP-012 | Task record 排除 raw stream、secret 与 terminal buffer。 | 当前 Task/store surface PASS | Release public surface 不能调用 raw writer。Task 与 Outbox 单 payload 上限为 256 KiB，单次 commit 上限为 1 MiB，且 transaction 前校验。Raw input response 只存在 private typed dispatch row，Task event 与 receipt 只含 digest。精确边界与超限 test 证明 rejection 零 mutation。 |
| TEP-013 | Corrupt/incompatible history fail closed 且可 inspect。 | PARTIAL | Schema/replay fail closed；只读脱敏 `admin inspect` 覆盖 newer schema、checksum、foreign-key 与 truncated database，且检查前后 bytes 不变。自动 quarantine 仍缺。 |
| TEP-014 | Provider `SessionStore` 与 Task storage 保持分离。 | PASS | Gateway SQLite 使用独立 crate/store 与 schema。 |
| TEP-015 | Final storage engine 与 durability profile 已批准。 | Scope decision PASS | ADR-S1 接受 SQLite WAL、`synchronous=FULL`、单 writer 与 local filesystem。Exact-candidate power-loss 和 operator evidence 仍是独立的 Phase 0/exit Gate。 |

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

1. TEP-001 至 TEP-014 全部 PASS；TEP-015 已不再是 decision blocker，但其明确列出的
   exact-candidate durability Gate 仍须通过。
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
- 把 process PID 或 lease timeout 当成 completion，可能重复 side effect。
- 允许 presenter 或 callback 修改 approval state，会造成 split-brain authorization。
- 当前 task-only baseline 不声称 production execution proof。若把通用 ledger foundation 当成
  universal Broker，Shell、Skill、MCP、扩展与 legacy effect 仍会落在 policy 之外。
