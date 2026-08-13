# Phase 0 Protocol Contracts 验收报告

[English](acceptance.md) | [设计](design_zh.md) |
[规划集](../../README_zh.md)

## 基线结论

**Leaf-contract 切片已通过，Phase 0 实现退出条件尚未达到。** 本报告覆盖基于
`6c115aefe04ace0d169a24fa7cd55ad7c1befa52` 的实现 worktree。Worktree 还包含 Task
reducer、SQLite Task storage、Runtime primitive 与 process-local Capability Broker
切片，但不表示完整 schema、fixture、coordinator/port integration、durable Broker
authority 或完整 ACP bridge 已存在。

新的 side-effect-free package 提供中立 Task、Runtime、Capability、Approval、
execution、header 与 error type。Deserializer 会校验 typed ID、schema version、
envelope kind、bounded text、opaque value、digest 与 error code。单个 scalar field
已 bounded，但 aggregate `Vec` 与 envelope collection total 尚未设上限，因此不构成
完整 bounded-input 声明。

## 已审计证据

| 来源 | 已核实的基线行为 |
| --- | --- |
| [`protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs#L9) 中的 `CONTROL_PROTOCOL_VERSION`、`InputMessage`、`OutputMessage` | Exact version 为 `1` 的产品专用 Shell/Core protocol，不是 ACP |
| [`AgentAdapter`](../../../../../crates/cosh-shell/src/adapter/mod.rs#L87)、[`AgentRunHandle`](../../../../../crates/cosh-shell/src/adapter/mod.rs#L107) 与 [`AgentEvent`](../../../../../crates/cosh-shell/src/types/mod.rs#L402) | 已有 Shell-local Agent lifecycle abstraction |
| [`session.rs`](../../../../../crates/cosh-core/src/session.rs#L83) 中的 `PersistedSession`、`SessionError` | 已有 versioned provider-session envelope 与 typed error |
| [`types/audit.rs`](../../../../../crates/cosh-shell/src/types/audit.rs#L29) 中的 `AuditIdentity` | 已有多个 correlation string，但没有 Task 或 Execution identity |
| [`cosh-gateway-contracts`](../../../../../crates/cosh-gateway-contracts/src/lib.rs) 与其 [manifest](../../../../../crates/cosh-gateway-contracts/Cargo.toml) | Side-effect-free leaf crate 只依赖 workspace `serde`、`thiserror` 与 `uuid`；不依赖 ACP、transport、async、storage 或 OS |
| [`task.rs`](../../../../../crates/cosh-gateway-contracts/src/task.rs) 与 [`runtime.rs`](../../../../../crates/cosh-gateway-contracts/src/runtime.rs) | Versioned command/event envelope 与中立 Task/Runtime payload 已公开并带 rustdoc |
| [`capability.rs`](../../../../../crates/cosh-gateway-contracts/src/capability.rs) 与 [`error.rs`](../../../../../crates/cosh-gateway-contracts/src/error.rs) | Capability request/decision/permit 与 bounded machine-readable error 已实现 |
| [`aggregate.rs`](../../../../../crates/cosh-gateway/src/task/aggregate.rs) 与 [`task_store.rs`](../../../../../crates/cosh-gateway/src/storage/task_store.rs) | Task transition、revision check、terminal guard、transactional event/projection/receipt/Outbox write 与 idempotency replay/conflict test 已存在 |
| [`capability/broker.rs`](../../../../../crates/cosh-gateway/src/capability/broker.rs) 与 [`capability/memory.rs`](../../../../../crates/cosh-gateway/src/capability/memory.rs) | Broker-facing policy branch 与 process-local atomic single-use permit check 已存在；approval 与 authority 不持久 |
| [已实现 Runtime contract](../../../../../docs/design/runtime-contracts.md) | 当前 JSONL negotiation 与 process path 仍是兼容输入 |

审计覆盖 source、dependency direction、rustdoc、serialization test 与 targeted
package validation。没有调用 provider、访问 ECS 或修改 host。

## 验收矩阵

| ID | 要求 | 基线 | 通过所需证据 |
| --- | --- | --- | --- |
| PC-01 | 中立 Task command/event type 位于已接受的 side-effect-free owner | 部分 | Rust type、rustdoc 与 dependency direction 通过；ownership ADR 与 schema fixture 尚未完成 |
| PC-02 | Runtime Port type 不含 ACP、cosh-core、Shell、HTTP 或渠道 type | 部分 | 中立 Runtime command/event type 通过；behavioral port 与 API review 尚未完成 |
| PC-03 | Capability request、permit、approval 与 execution outcome 全部 typed | 部分 | Public serde type 与八个 Broker-facing test 在 policy 前 pin target/descriptor/完整 operation digest/scope；trusted canonicalizer test、golden schema、durable approval 与 execution-result lifecycle 尚未完成 |
| PC-04 | Product schema version 与 ACP、Core version 相互独立 | 部分 | 显式 schema constant 与 fail-closed version/type test 通过；compatibility manifest 尚未完成 |
| PC-05 | Task reducer 保证 monotonic revision 和一个 terminal Run event | 部分 | Critical revision-gap 与 terminal-guard test 通过；duplicate/reorder property matrix 尚未完成 |
| PC-06 | Command idempotency 定义同 key 同 digest replay 与 conflict | 部分 | SQLite integration replay 同 actor/key/digest，并拒绝 changed digest；authenticated ingress scope 与 fixture corpus 尚未完成 |
| PC-07 | Cancellation 持久化 intent，并确定性处理 completion race | 部分 | Cancellation intent/terminal fact 与 reducer guard 已存在；fake-runtime completion-race fixture 尚未完成 |
| PC-08 | Error bounded、redacted、stable 且 machine-readable | 部分 | Scalar code/message construction 与 deserialization 通过；aggregate collection cap 和 secret-scanner adversarial fixture 尚未完成 |
| PC-09 | ACP v1 baseline 与 capability negotiation 有 fixture | 缺失 | 官方 SDK 生成的 initialize/session fixture |
| PC-10 | 现有 Shell/Core JSONL v1 保持兼容 | 仅基线已就绪 | 现有 protocol suite 与新 CoshCore bridge fixture |
| PC-11 | Unknown version 与 unsupported capability fail closed | 部分 | Unknown Gateway schema version 与 envelope-kind mismatch 已 fail closed；ACP fake-Agent negative test 尚未完成 |
| PC-12 | 中英文 design/acceptance pair 语义等价 | 本次文档检查后就绪 | 下方记录的 parity review |

“部分”表示 leaf source 或已有先例只覆盖部分要求，最后一列列出的证据仍需补齐。

## 必要 Fixture 清单

在仓库包含下列等价 versioned fixture 前，Phase 0 实现不能退出：

```text
fixtures/gateway-contracts/v1/
  gateway-command-create-task.json
  gateway-command-idempotency-conflict.json
  task-event-run-lifecycle.jsonl
  task-event-approval-execution.jsonl
  runtime-command-prompt.json
  runtime-event-message-tool-permission.jsonl
  capability-request.json
  execution-permit.json
  contract-error.json
  malformed/
    unknown-schema-version.json
    oversized-content.json
    cross-task-correlation.json
fixtures/acp/v1/
  initialize-minimal.jsonl
  initialize-capabilities.jsonl
  prompt-cancel.jsonl
  permission-terminal.jsonl
fixtures/cosh-core-bridge/v1/
  initialize-and-turn.jsonl
  approval-and-host-execution.jsonl
```

以上路径仍是建议 artifact layout，leaf-contract 切片未提供这些文件。

## 必要验证命令

后续实现验收必须记录以下等价命令与 count：

```bash
cargo test --package cosh-gateway-contracts
cargo test --package cosh-gateway task_reducer
cargo test --package cosh-gateway --test contract_fixtures
cargo test --package cosh-shell --test protocol
```

还必须提供：

- 所有 positive 与 malformed fixture 的 JSON Schema validation；
- 证明 domain contract 不依赖 ACP 或 transport crate 的 dependency graph；
- 自动生成的 compatibility manifest，把 ACP wire `1`、实际 ACP SDK package
  version、Gateway schema `1` 与 Core control protocol `1` 作为不同值记录；
- 当前 Shell/Core protocol fixture 仍然通过的 diff evidence。

本切片记录的 targeted leaf-crate validation：

```text
cargo fmt --package cosh-gateway-contracts -- --check
cargo test --locked --package cosh-gateway-contracts
cargo clippy --locked --package cosh-gateway-contracts --all-targets -- -D warnings
cargo doc --locked --package cosh-gateway-contracts --no-deps
cargo tree --locked --package cosh-gateway-contracts --edges normal
result: 6 integration tests passed；unit 与 doc-test target passed
dependency result：仅 serde、thiserror 与 uuid
```

## 未实现项

- 没有完整 Gateway schema/golden-fixture corpus 或 aggregate collection limit。
- 没有贯通 Runtime、Capability Broker、Presentation 与 storage port 的 integrated
  Task coordinator 和 behavioral mapping。
- 没有 durable approval/permit/execution Broker ledger 或 reconciliation path；
  当前 Broker store 只在进程内有效。
- 官方 ACP Rust SDK、codec 与 fake-Agent test 已存在于 library boundary；canonical
  versioned fixture corpus 与 real-adapter 证据仍不完整。
- 没有把当前 JSONL 转成中立 event 的 CoshCore Bridge。
- 没有 compatibility manifest 或 rollout feature flag。

## Exit Criteria

只有满足下列条件，Phase 0 protocol contract 才能通过：

1. PC-01 至 PC-12 在同一个准确 commit 上都有实现证据。
2. 所有必要 schema 与 fixture 已评审并 versioned。
3. State 与 cancellation property test 确定性通过。
4. ACP fixture 通过 pinned official Rust SDK 产生或消费，并单独断言
   `protocolVersion = 1`，不与 SDK version 混淆。
5. Task storage 不包含 external transport 或 runtime-specific type。
6. Security review 确认 bounded input 与 secret-free error behavior。
7. 现有 Shell/Core protocol target 保持通过。
8. 影响 public type 或 schema 的开放决策已通过 ADR 或 accepted design revision 关闭。

## 本切片的验证记录

- 中英文文件包含规定的双向链接。
- Command block 与 schema 名在两种语言中完全一致。
- 已从当前目录检查相对源码链接。
- 已检查 Markdown whitespace 与 diff hygiene。
- 上述 targeted formatting、package test、Clippy、rustdoc 与 dependency audit
  已通过。
- 由于该 crate 没有 I/O 或 host behavior，有意跳过 ECS、provider 与 host-mutation
  validation。
