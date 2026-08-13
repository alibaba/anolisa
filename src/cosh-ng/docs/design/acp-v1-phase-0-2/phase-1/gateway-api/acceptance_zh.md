# Phase 1 Gateway API 验收基线

[English](acceptance.md) | [设计](design_zh.md)

## 基线结果

**整体结果：基于上游 `e3763b001c91f3c13dc6afbd57aac924162e9f59` 的候选实现为 PARTIAL，
Phase 1 仍是 NOT ACCEPTED。** 候选实现加入带 peer-UID authentication 的 bounded local Unix
daemon/client，以及 durable Task submit/get/events/cancel。Runtime scheduling、approval resolution、
Outbox delivery、restart reconciliation、remote identity、channel adapter 与 real-provider evidence 仍缺。

本文记录实现前 readiness，不能解读为 Phase 1 已验收通过。

## 结果口径

| 结果 | 含义 |
| --- | --- |
| PASS | 固定提交上的证据满足该验收项。 |
| FAIL | 已有实现，但行为违反该验收项。 |
| NOT IMPLEMENTED | 所需 production path 不存在。 |
| BLOCKED | 在指定外部决策或依赖完成前无法继续验证。 |

## 已检查证据

- 上游基线：`e3763b001c91f3c13dc6afbd57aac924162e9f59`。
- [`cosh-types/output.rs`](../../../../../crates/cosh-types/src/output.rs) 定义当前 CLI response
  envelope。
- [`cosh-cli/main.rs`](../../../../../crates/cosh-cli/src/main.rs) 直接 dispatch 当前 command module。
- [`cosh-core/protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs) 定义内部 Shell/Core
  JSONL protocol。
- [`cosh-core/session_control.rs`](../../../../../crates/cosh-core/src/session_control.rs) 管理
  provider session，而不是 Task。
- 候选源码加入 private versioned local API、daemon、typed client、installed CLI route 与
  SQLite-backed Task projection，不开放 remote listener。

## 验收矩阵

| ID | 验收项 | 基线 | 证据或缺失产物 |
| --- | --- | --- | --- |
| GWA-001 | 带版本、有长度上限的本地 API 接收 typed Task command。 | PARTIAL | 已有 local submit/get/events/cancel 与 bounded framing；approval/append/retry 和 frozen golden corpus 仍缺。 |
| GWA-002 | Transport identity 覆盖不可信 actor body。 | PARTIAL | Request 不携带 actor，Unix peer UID 为 authority；tenant/remote identity resolution 仍缺。 |
| GWA-003 | Handler code 不具备 OS、PTY、process spawn、Agent 或 store 能力。 | PARTIAL | Handler 不具备 Runtime/PTY/OS execution；当前 daemon 直接拥有 local store，target port split 尚未完成。 |
| GWA-004 | 所有 mutation 均通过 `TaskCommandPort`。 | PARTIAL | Mutation 统一进入 daemon service path，但还没有独立强制的 `TaskCommandPort` boundary。 |
| GWA-005 | `TaskCoordinator` 是 Task aggregate 唯一 writer。 | PARTIAL | Local service 串行化 Task write；scheduling/recovery coordinator 仍缺。 |
| GWA-006 | 同 request、同 digest 重放返回原 receipt。 | PARTIAL | Durable command receipt 支持 submit/cancel；仍缺 commit 后丢 response 的 crash/retry evidence。 |
| GWA-007 | 同 request、不同 digest 确定性失败。 | PARTIAL | Store 已有 conflict behavior；仍缺 end-to-end local API fixture。 |
| GWA-008 | Task read 和有界 event page 均执行 tenant authorization。 | PARTIAL | Peer UID 约束 Task read，page 有 hard limit；tenant authorization 未实现。 |
| GWA-009 | Approval resolution 不能创建或扩大 permit。 | NOT IMPLEMENTED | Approval endpoint 与 Broker 不存在。 |
| GWA-010 | Outbox delivery 容忍重复发送与重启。 | NOT IMPLEMENTED | 无 outbox consumer。 |
| GWA-011 | 现有 Shell/Core JSONL 不作为 Gateway API 暴露。 | PASS | 它仍只位于 runtime code。 |
| GWA-012 | Daemon 禁用时现有 CLI 行为保持可用。 | 源码切片 PASS | 现有 `doctor` 与 `run` 不依赖 `serve`/`task`。 |
| GWA-013 | Phase 1 禁止 remote listener。 | 源码切片 PASS | 只存在 local Unix listener。 |
| GWA-014 | 已选择跨渠道 identity authority。 | BLOCKED | Product/security owner 决策未完成。 |

## 实现验收要求的 fixture 与命令

实现报告必须在未来 Gateway test owner 下保留以下产物：

| Fixture/产物 | 目的 |
| --- | --- |
| `gateway-v1/*.json` golden corpus | 覆盖合法、非法、超限、未知版本请求与响应。 |
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

包含本报告的 Stage 6 commit 已通过 Rust 1.88 验证：

- `cargo +1.88 test -p cosh-gateway --no-fail-fast`：126 个 library、4 个 binary 和 7 个
  installed-CLI 测试通过，0 失败。
- `cargo +1.88 clippy -p cosh-gateway --all-targets -- -D warnings` 通过。
- `cargo +1.88 doc -p cosh-gateway --no-deps` 通过。
- Focused fixture 覆盖 peer/server UID authentication、installation binding、bounded framing、
  SQL event page、strict field、replay、queued cancel、safe stale socket 与 installed CLI parsing。
- Local built-binary smoke 通过 Unix API 完成 `serve`、`task submit`、单页 `events`、queued
  `cancel` 与 SIGINT socket cleanup。

本报告不声称完成 real provider、ECS、remote transport、manual Terminal、commit 后丢响应的
crash fixture、audit evidence sink 或 screenshot 验证。

## Exit criteria

Phase 1 Gateway API 只有满足以下条件才算通过：

1. GWA-001 至 GWA-013 全部 PASS；GWA-014 有正式决策，或由 owner 批准明确 local-only scope。
2. Handler-boundary test 证明 Gateway handler 不能执行 OS 工作。
3. Crash/retry fixture 证明持久幂等和 transactional outbox 行为。
4. Security review 覆盖 peer credential、tenant/actor binding、target substitution、replay、resource
   limit、redaction 与 approval authorization。
5. 验收报告记录 exact commit、command、test count、artifact 与未测试的 external-channel path。

## 当前风险

- 直接复用 `CoshResponse<T>` 可能混淆 CLI execution 与 asynchronous Task receipt。
- 复用 Shell/Core JSONL contract 会把 runtime assumption 泄漏到 public ingress。
- 在 Task idempotency 前增加 channel handler，会使弱网 retry 不安全。
- 把 local single-user deployment 当作无 identity 环境，会令后续 remote migration 产生安全破坏性变更。
