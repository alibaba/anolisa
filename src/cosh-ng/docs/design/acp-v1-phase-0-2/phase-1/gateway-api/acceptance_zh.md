# Phase 1 Gateway API 验收基线

[English](acceptance.md) | [设计](design_zh.md)

## 基线结果

**整体结果：`6c115aefe04ace0d169a24fa7cd55ad7c1befa52` 上为 NOT IMPLEMENTED。** 现有 JSON
envelope 和相关联的 control message 可以复用，但不存在 Gateway API、Task command port、ingress
identity、持久幂等或 projection delivery path。

本文记录实现前 readiness，不能解读为 Phase 1 已验收通过。

## 结果口径

| 结果 | 含义 |
| --- | --- |
| PASS | 固定提交上的证据满足该验收项。 |
| FAIL | 已有实现，但行为违反该验收项。 |
| NOT IMPLEMENTED | 所需 production path 不存在。 |
| BLOCKED | 在指定外部决策或依赖完成前无法继续验证。 |

## 已检查证据

- 基线：`git rev-parse HEAD` 返回
  `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`。
- [`cosh-types/output.rs`](../../../../../crates/cosh-types/src/output.rs) 定义当前 CLI response
  envelope。
- [`cosh-cli/main.rs`](../../../../../crates/cosh-cli/src/main.rs) 直接 dispatch 当前 command module。
- [`cosh-core/protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs) 定义内部 Shell/Core
  JSONL protocol。
- [`cosh-core/session_control.rs`](../../../../../crates/cosh-core/src/session_control.rs) 管理
  provider session，而不是 Task。
- 仓库搜索没有发现 `GatewayApi`、`IngressPort` 或 `TaskCommandPort` 实现。

## 验收矩阵

| ID | 验收项 | 基线 | 证据或缺失产物 |
| --- | --- | --- | --- |
| GWA-001 | 带版本、有长度上限的本地 API 接收 typed Task command。 | NOT IMPLEMENTED | 无 daemon/API module。 |
| GWA-002 | Transport identity 覆盖不可信 actor body。 | NOT IMPLEMENTED | 无 identity resolver 或 ingress envelope。 |
| GWA-003 | Handler code 不具备 OS、PTY、process spawn、Agent 或 store 能力。 | NOT IMPLEMENTED | 无可检查的 handler boundary。 |
| GWA-004 | 所有 mutation 均通过 `TaskCommandPort`。 | NOT IMPLEMENTED | Port 不存在。 |
| GWA-005 | `TaskCoordinator` 是 Task aggregate 唯一 writer。 | NOT IMPLEMENTED | Task aggregate 不存在。 |
| GWA-006 | 同 request、同 digest 重放返回原 receipt。 | NOT IMPLEMENTED | 无持久 idempotency table。 |
| GWA-007 | 同 request、不同 digest 确定性失败。 | NOT IMPLEMENTED | 无 request ledger。 |
| GWA-008 | Task read 和有界 event page 均执行 tenant authorization。 | NOT IMPLEMENTED | 无 projection/event API。 |
| GWA-009 | Approval resolution 不能创建或扩大 permit。 | NOT IMPLEMENTED | Approval endpoint 与 Broker 不存在。 |
| GWA-010 | Outbox delivery 容忍重复发送与重启。 | NOT IMPLEMENTED | 无 outbox consumer。 |
| GWA-011 | 现有 Shell/Core JSONL 不作为 Gateway API 暴露。 | PASS | 它仍只位于 runtime code。 |
| GWA-012 | Daemon 禁用时现有 CLI 行为保持可用。 | PASS | 尚无 daemon integration。 |
| GWA-013 | Phase 1 禁止 remote listener。 | PASS | Listener 不存在；实现后必须保持此属性。 |
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

本次**没有运行**这些命令，因为候选 package 尚无 Gateway API 实现或对应 test target。现有 package
suite 只验证其他候选切片；本模块仍未实现，文档检查只验证其链接与双语等价性。

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
