# Phase 1 Gateway API 验收基线

[English](acceptance.md) | [设计](design_zh.md)

## 基线结果

**整体结果：基于 `e90d9d9402c7fa1c8122267eb4e075c0adda51f5` 的候选实现为 PARTIAL，
Phase 1 仍是 NOT ACCEPTED。** 候选实现加入带 peer-UID authentication 的 bounded local Unix
daemon/client、handler port、durable Task submit/get/events/cancel/retry/append-input/resolve-approval、Outbox
scheduling 与 restart convergence。Production `serve` 只接收 `core/gateway-brokered-v1`，其受约束的
task-only inventory 只有 production tool `ask_user_question`；没有 production `ExecutionTarget`，也不
依赖 checkpoint/ws-ckpt。`doctor` 与 `run` 保持明确不受治理的 ACP interoperability。Remote/multi-tenant
identity、channel adapter、已验收的真实 Codex/Claude evidence 与人工 Terminal 验证仍缺。

Phase 1 限定为 installation-scoped 单用户环境。Durable `InstallationId` 加已认证 local peer UID 构成
v1 authorization boundary。`TenantId`、cross-tenant authority 与 remote identity 留给未来 v2。

## 结果口径

| 结果 | 含义 |
| --- | --- |
| PASS | 固定提交上的证据满足该验收项。 |
| FAIL | 已有实现，但行为违反该验收项。 |
| NOT IMPLEMENTED | 所需 production path 不存在。 |
| BLOCKED | 在指定外部决策或依赖完成前无法继续验证。 |

## 已检查证据

- 规划基线：`e90d9d9402c7fa1c8122267eb4e075c0adda51f5`。
- [`cosh-types/output.rs`](../../../../../crates/cosh-types/src/output.rs) 定义当前 CLI response
  envelope。
- [`cosh-cli/main.rs`](../../../../../crates/cosh-cli/src/main.rs) 直接 dispatch 当前 command module。
- [`cosh-core/protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs) 定义内部 Shell/Core
  JSONL protocol。
- [`cosh-core/session_control.rs`](../../../../../crates/cosh-core/src/session_control.rs) 管理
  provider session，而不是 Task。
- 候选源码加入 private versioned local API、daemon、typed client、installed CLI route 与
  SQLite-backed Task projection，不开放 remote listener。
- [`daemon/handler.rs`](../../../../../crates/cosh-gateway/src/daemon/handler.rs) 只依赖已解析 actor、
  admission value、`TaskCommandPort` 与 `TaskProjectionPort`。
- [`daemon/tests.rs`](../../../../../crates/cosh-gateway/src/daemon/tests.rs) 覆盖 frozen wire、response
  loss、精确 frame bound、forbidden handler import 与 250 ms connection quantum。

## 验收矩阵

| ID | 验收项 | 基线 | 证据或缺失产物 |
| --- | --- | --- | --- |
| GWA-001 | 带版本、有长度上限的本地 API 接收 typed Task command。 | 本地 v1 surface PASS | Frozen Gateway wire v1 覆盖全部 enabled command，包括准确 pending input append 与 fenced retry，并覆盖 exact/oversized frame test。Remote ingress 留给 Phase 2。 |
| GWA-002 | Transport identity 覆盖不可信 actor body。 | PARTIAL | Request 不携带 actor；`InstallationId` 加 Unix peer UID 是 authority，forged identity field fail。Tenant/remote identity 明确留给未来 v2。 |
| GWA-003 | Handler code 不具备 OS、PTY、process spawn、Agent、store、scheduler 或 Runtime 能力。 | PASS | `daemon/handler.rs` 只 import contracts 与两个 port；source-boundary test 拒绝 forbidden dependency。 |
| GWA-004 | 所有 enabled mutation 均通过 `TaskCommandPort`。 | PASS | Submit、cancel、retry、append-input 与 resolve-approval 经 command port；get/events 经 `TaskProjectionPort`。 |
| GWA-005 | `TaskCoordinator` 是 Task aggregate 唯一 writer。 | PARTIAL | Local service 与 scheduler settlement 使用 coordinator；仍需针对未来 adapter 做最终 ownership audit。 |
| GWA-006 | 同 idempotency key、同 digest 重放原 receipt。 | PASS | Raw Unix response-loss fixture 丢弃第一个 submit response 后 retry 并返回 durable 原记录；cancel replay 也有覆盖。 |
| GWA-007 | 同 idempotency key、不同 digest 确定性失败。 | PASS | End-to-end local API fixture 返回不可恢复的 `idempotency_conflict`。 |
| GWA-008 | Task read 与有界 event page 执行 authorization。 | PARTIAL | Installation-derived local actor 与 peer UID 约束读取，foreign actor 得到 not-found，page 有界。Tenant authorization 未实现且不作声明。 |
| GWA-009 | Approval resolution 不能创建或扩大 permit。 | PARTIAL | 异步 endpoint 提交有绑定的 terminal approval。通用 Permit/Execution contract 作为后续基础保留，production 没有 ExecutionTarget 或 checkpoint loop；通用 presentation/Broker coverage 仍缺。 |
| GWA-010 | Outbox delivery 容忍重复发送与重启。 | PARTIAL | Task-only slice 保留 scheduler claim/reclaim/ack 与稳定 Delivery ID；不声称 checkpoint-specific execution 或 result replay。通用 delivery、remote path 与 power-loss evidence 仍开放。 |
| GWA-011 | 现有 Shell/Core JSONL 不作为 Gateway API 暴露。 | PASS | 它仍只位于 runtime code。 |
| GWA-012 | Daemon 禁用时 compatibility behavior 保持可用。 | PASS | Standalone Shell 是 rollback path。`doctor`/`run` 独立且属于 ungoverned ACP interop，不是 governed claim。 |
| GWA-013 | Phase 1 禁止 remote listener。 | 源码切片 PASS | 只存在 local Unix listener。 |
| GWA-014 | 已选择 Phase 1 identity authority 与未来 cross-channel boundary。 | Scope decision PASS | v1 使用 `InstallationId` 加 local peer identity；remote channel、`TenantId` 与 multi-tenant authorization 要求 v2。 |

## 实现验收要求的 fixture 与命令

实现报告必须在未来 Gateway test owner 下保留以下产物：

| Fixture/产物 | 目的 |
| --- | --- |
| `gateway-wire-v1.json` golden corpus | 覆盖全部 enabled command，并配套 strict invalid、oversized 与 version test。 |
| `idempotency-replay` crash fixture | Commit command 后丢弃 response，再 retry 并比较 receipt。 |
| `forged-actor` fixture | 证明 body identity 不能覆盖 peer/channel identity。 |
| `handler-boundary` dependency test | Import execution、PTY、process、store 或 Agent bridge 时失败。 |
| `outbox-redelivery` fixture | 在 send 与 ack 之间重启，证明 Delivery ID 稳定。 |

代码存在后预期执行以下 scoped command：

```bash
cargo test --package cosh-gateway gateway_api
cargo test --package cosh-gateway gateway_contract
cargo test --package cosh-gateway-contracts gateway_schema
```

Focused daemon suite 覆盖 peer/server UID authentication、installation binding、全部 enabled wire
command、精确最大与 oversized frame、SQL event page、strict field、response-loss replay 与 digest
conflict、cancellation、safe stale socket、handler import boundary，以及 idle/partial-frame client 在
一个 250 ms admission quantum 后退出、scheduler 与后续合法请求继续推进。使用
`cargo test --locked --package cosh-gateway daemon --no-fail-fast` 复现。

本报告不声称完成已验收的真实 Codex/Claude、ECS、remote transport、人工 Terminal、完整的
commit 后丢响应 crash matrix 或 screenshot 验证。Task-only inventory 只暴露 `ask_user_question`；
没有 production ExecutionTarget 或 checkpoint/ws-ckpt path 作为本报告证据。

## Exit criteria

Phase 1 Gateway API 只有满足以下条件才算通过：

1. GWA-001 至 GWA-013 全部 PASS；GWA-014 记录明确的 installation-scoped owner 决策。
2. Handler-boundary test 证明 Gateway handler 不能执行 OS 工作。
3. Crash/retry fixture 证明持久幂等和 transactional outbox 行为。
4. Security review 覆盖 peer credential、installation/actor binding、target substitution、replay、resource
   limit、redaction 与 approval authorization。
5. 验收报告记录 exact commit、command、test count、artifact 与未测试的 external-channel path。

## 当前风险

- 直接复用 `CoshResponse<T>` 可能混淆 CLI execution 与 asynchronous Task receipt。
- 复用 Shell/Core JSONL contract 会把 runtime assumption 泄漏到 public ingress。
- 在 Task idempotency 前增加 channel handler，会使弱网 retry 不安全。
- 把 v1 installation boundary 当作 cross-tenant authority 属于安全错误；multi-tenant semantic 必须采用新版本并重新评审。
