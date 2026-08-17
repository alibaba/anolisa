# 持久 Task Plane

[English](durable-task-plane.md)

关联架构：[COSH Gateway 与 ACP 架构](README_zh.md)

## 目的

Task Plane 是 Task 状态、调度意图、Runtime ownership 与 delivery 的持久协调者。
生命周期决策只以它为事实来源，内存 worker 和 Provider process 都不具有权威性。

## 持久记录

持久化模型区分：

- append-only Task event；
- 当前 Task projection 与 revision；
- idempotency receipt；
- Outbox delivery intent；
- Run lease 与 Runtime binding；
- Approval、input、execution 与 Runtime-dispatch ledger；
- 有界 audit 与 reconciliation evidence。

每条记录携带准确内部 identity。Raw provider output 和 secret 不进入 Task history。

## 原子 command 边界

一个 Task command 在同一 transaction 内提交：

1. expected-revision 与 idempotency 校验；
2. 被接受的 Task event；
3. reducer 生成的 projection；
4. 持久 command receipt；
5. 状态转换要求的 Outbox intent。

以上内容必须全部提交或全部不提交。Writer 在 transaction 修改存储前检查 event 数、
Outbox 数、单 payload 大小和完整 command aggregate 大小。

## Outbox

Outbox 是状态转换与外部 delivery 之间的持久边界。

- Stable Delivery ID 标识一次逻辑发送。
- Claim 与 acknowledgement 是两个持久转换。
- 响应丢失时复用相同 delivery identity。
- Malformed 或永久拒绝的 entry 进入有界 dead-letter，不得崩溃或 busy-loop daemon。
- Delivery 不授予执行权限。Authority 只来自 Approval、Permit 与 Execution record。

## Run lease 与 Runtime binding

Worker 启动或 poll Runtime 前必须持有 current lease。Lease 包含 owner、generation、
revision 与 expiry。Renewal 改变 revision，takeover 改变 generation 并 fence 旧 worker。

第一次 prompt 前，worker 持久化绑定当前 lease generation 的 RuntimeBinding。所有
callback 与 authority transition 都验证 current Run、binding 与 fence。

## 重启与 takeover

Gateway 重启后依据持久状态分类，不猜测旧 process 已经做了什么：

- 带有效 Outbox intent 的 queued work 可以重新 claim；
- 无法重建可信 admission 的 work fail closed；
- 未确认 Runtime start 按实际越过的持久边界收敛为 known failure 或 uncertain；
- pending input 和 approval ledger 在 Task terminalization 前收敛；
- Started execution 或 delivery 进入 `Uncertain`/`Unknown`，不得自动 replay；
- typed target 能证明准确 operation outcome 时，reconciliation 可以把 uncertain 结果
  转为 conclusive result。

显式 release 与 lease expiry 必须区分，使中断的 takeover 可以继续，同时避免反复领取已
收敛的 suspended Run。

## Cancellation、retry 与 input

- Cancellation 必须先持久化，再发 process signal。
- 安全静止的 suspended Run 可以原子放弃；uncertain effect 不能伪装成从未发生的 cancel。
- Retry 仅在旧 lease、binding、input、approval 和 delivery 全部静止后创建新 Run。
- Input append 绑定准确 Task、Run、Runtime request、revision 与 digest。
- Shutdown 在关闭 binding 前先收敛 pending input 和 approval。

## Storage 与 recovery contract

- 本地 single-writer profile 使用 SQLite WAL 与 FULL durability。
- Schema migration 带 checksum；newer/divergent history 必须无 mutation 拒绝。
- Startup 在接收工作前检查 integrity 与 foreign key。
- Backup 是 online、source-bound、no-clobber，并在发布前验证。
- Restore 只写新路径，并检查 installation identity 与 schema。
- Operator inspect 为 read-only，只返回有界脱敏 health。

Filesystem authority 必须跨 validation 与 open 保持；只检查 pathname 不能防止 rename 或
symlink race。

## Failure 语义

| 边界 | Recovery 分类 |
| --- | --- |
| Task transaction commit 前 | Command 未被接受 |
| Commit 后、Outbox send 前 | 重新 claim stable delivery |
| Receiver 可能已接受、无 durable ack | Delivery `Unknown`；不得盲目 resend |
| Permit claimed、无 start audit | 只有 audit gate 能证明时才是 known no effect |
| Execution Started、无 conclusive result | Execution `Uncertain`；reconcile 或 suspend |
| Result committed、API response 丢失 | Replay durable result 与 receipt |

## 验收不变量

- Crash 后 event、projection、receipt 与 Outbox 不得分叉。
- Stale lease owner 不能写 Task、Runtime、approval、input 或 execution 状态。
- Response-loss replay 不重复 effect 或 Runtime delivery。
- Poison data 不得饿死无关 work。
- 每个 uncertain effect 在准确 reconcile 或 operator settlement 前保持可见。
