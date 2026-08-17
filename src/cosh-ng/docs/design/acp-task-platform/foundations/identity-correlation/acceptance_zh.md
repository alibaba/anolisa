# Phase 0 Identity and Correlation 验收报告

[English](acceptance.md) | [设计](design_zh.md) |
[规划集](../../README_zh.md)

## 基线结论

**Typed leaf identity 切片已通过，G0 退出条件尚未达到。** 实现 worktree 基于
`6c115aefe04ace0d169a24fa7cd55ad7c1befa52`。

新的 contract crate 增加不同的 validated internal ID、`Correlation`、bounded
`ExternalRef`、actor/target reference，以及包含 instance 与 generation 的 Runtime
binding。Gateway storage 现在会持久化 Task owner、typed event identity 与
actor-scoped idempotency receipt，Broker 会校验完整 authoritative Actor provenance。
它仍不包含 actor registry、durable external-reference mapping、完整 Runtime/capability
relation 或 active Runtime-generation admission fence。

## 已审计证据

| 来源/符号 | 已核实事实 |
| --- | --- |
| [`ProviderSessionId::parse`](../../../../../crates/cosh-core/src/session.rs#L28) | 构造 path 前拒绝 non-canonical provider-session UUID |
| [`PersistedSession.workspace_scope`](../../../../../crates/cosh-core/src/session.rs#L83) | Provider history 绑定 canonical workspace |
| [`AuditIdentity`](../../../../../crates/cosh-shell/src/types/audit.rs#L29) | 当前 audit correlation 使用 optional string field |
| [`ShellCommandAuditIdentity`](../../../../../crates/cosh-shell/src/types/mod.rs#L55) | Shell handoff 分开携带 Run、request 与 tool reference |
| [`ProviderToolKey`](../../../../../crates/cosh-shell/src/runtime/provider_tool_state.rs#L236) | Tool state 在内存中通过 Run 限定 tool ID scope |
| [`RunCommand`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service/command.rs#L21) | Core service 携带 Run 与 session scope，但没有 durable binding generation |
| [`ids.rs`](../../../../../crates/cosh-gateway-contracts/src/ids.rs) | 16 个 prefixed internal ID newtype 共享 canonical generation、parsing、serde 与 cross-type rejection |
| [`common.rs`](../../../../../crates/cosh-gateway-contracts/src/common.rs) | `Correlation`、`ActorRef`、`RuntimeBindingRef`、digest 与 bounded value 已类型化 |
| [`external.rs`](../../../../../crates/cosh-gateway-contracts/src/external.rs) | External namespace、authority、scope digest 与 bounded opaque value 分离表示 |
| [`task_store.rs`](../../../../../crates/cosh-gateway/src/storage/task_store.rs) | Task owner、event、projection 与 actor-scoped key 加 payload-digest receipt 在一个 transaction 中提交；包含 replay/conflict 与 actor-substitution test |
| [`capability/broker.rs`](../../../../../crates/cosh-gateway/src/capability/broker.rs) | Request admission 比较完整 authoritative `ActorRef`，包括 ID、issuer、kind 与 assurance，并且不写 Task storage |

Targeted test 覆盖 ID canonicalization、cross-type parsing、serde validation、
envelope schema matching 与 size limit。Side-effect-free crate 不需要 provider、
ECS 或 host-mutation validation。

## 验收矩阵

| ID | 要求 | 基线 | 通过所需证据 |
| --- | --- | --- | --- |
| IC-01 | 所有 internal lifecycle ID 都是不同的 validated newtype | Leaf type 通过 | Constructor、canonical serde 与 cross-parse unit test 通过；G0 仍需 property fixture |
| IC-02 | Task、Run、Agent Session、Runtime binding、Approval、Permit、Execution、Delivery parent 被强制执行 | 部分 | Runtime binding、capability request 与 permit 携带 parent；database foreign-key 与 domain-constructor test 尚未完成 |
| IC-03 | Actor 来自 authenticated issuer/subject，不来自 request payload | 部分 | Broker 会根据 authoritative binding 拒绝完整 Actor provenance substitution；authenticated ingress/IdentityResolver test 尚未完成 |
| IC-04 | Channel reference 包含 adapter、authority、conversation 与 message scope | 部分 | Kind、authority、scope digest 与 opaque value 为必填；cross-tenant collision 与 retry fixture 尚未完成 |
| IC-05 | Provider 与 ACP ID 保持 opaque external reference | 部分 | External kind 与 bounded opaque value 已类型化；使用 arbitrary、colliding、non-UUID value 的 bridge test 尚未完成 |
| IC-06 | Runtime generation fence 能拒绝 stale child output | 部分 | Runtime binding 目前只携带 instance 与 generation；active admission fence 不存在，crash/restart delayed-event test 尚未完成 |
| IC-07 | Tool use 与 OS Execution identity 不混用 | 部分 | 不同 `ToolUseId` 与 `ExecutionId` newtype 通过 cross-parsing；multi-execution fixture 与 durable constraint 尚未完成 |
| IC-08 | Idempotency key reuse 校验 scoped payload digest | 部分 | SQLite test replay 同 actor/key/digest，并拒绝 another digest 或 actor；authenticated ingress scope 与 channel fixture 尚未完成 |
| IC-09 | External identity value bounded，并在 diagnostics 中 redacted | 部分 | Bounded construction 与 deserialization 已实现；injection、encryption/digest 与 log test 尚未完成 |
| IC-10 | Legacy provider-session 与 Shell identity 迁移时不猜测 Task identity | 仅设计 | Dual-mode migration fixture 与显式 gap output |
| IC-11 | 新字段的 audit schema change 经过显式评审 | 缺失 | Accepted audit compatibility decision 与 reader test |
| IC-12 | 中英文文档等价且链接可用 | 文档检查后就绪 | 已记录的 documentation check |

## 必要 Fixture 与 Artifact

```text
fixtures/identity/v1/
  internal-ids.json
  correlation-complete.json
  external-channel-ref.json
  external-provider-session-ref.json
  external-acp-refs.json
  runtime-binding-generation.json
  approval-permit-execution-chain.json
  legacy-correlation-gap.json
  malformed/
    wrong-prefix.json
    noncanonical-id.json
    cross-tenant-message.json
    cross-task-run.json
    stale-runtime-generation.json
    oversized-external-value.json
    actor-substitution.json
```

必要实现产物还包括：

- 记录 prefix、scope、allocator、lifetime 与 parent 的 ID registry；
- 包含 foreign key 与 scoped unique index 的 database DDL；
- 记录 external reference field 分别使用 raw、encrypted、digested 或 loggable
  形式的 data-classification 文档；
- 变更前后 reader 的 audit compatibility fixture；
- cosh-core、Shell 与 ACP ID 的准确 mapping table。

Typed source 已存在；上述 versioned fixture 与 durable artifact 尚未完成。

## 必要验证命令

最终 G0 验收必须包含下列等价 targeted command：

```bash
cargo test --package cosh-gateway-contracts identity
cargo test --package cosh-gateway identity_resolver
cargo test --package cosh-gateway runtime_fencing
cargo test --package cosh-gateway --test identity_storage
cargo test --package cosh-shell --test protocol
```

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

Storage、fencing 与 ingress test 添加后，报告必须保留 property-test seed 与
test count。

## 未实现项

- Task owner/event/storage identity relation 与 actor-scoped receipt 已存在；完整 actor
  registry、external-reference、Runtime-binding、Approval、Permit、Execution 与 Delivery
  relation 或 foreign key 仍缺失。
- 没有 actor mapping registry 或 authenticated identity resolver。
- 没有 Runtime event-admission fence；只有携带 generation 的 binding type。
- 没有 channel identity 或 scoped ingress idempotency。
- 没有 durable ACP Connection、Session、Request、Message、Tool Call 或 Terminal mapping；
  对应 external kind 只存在于 pure reference 中。
- 没有针对新 correlation field 的 accepted audit evolution。

## Exit Criteria

G0 identity acceptance 要求：

1. IC-01 至 IC-12 在一个记录准确的 implementation commit 上通过。
2. Prefix 与 UUID representation 已通过 ADR 冻结。
3. 所有 parent relation 都在 constructor 与 storage 中强制执行。
4. Runtime restart test 证明 stale output 不能修改 Task。
5. Channel replay 与 actor-substitution test fail closed。
6. ACP fixture 证明 external value 保持 opaque 与 connection-scoped。
7. Log、error 与 audit output 不包含 raw sensitive external identity。
8. Legacy record 显示 correlation gap，不伪造 identity。

## 本切片的验证记录

- 已提供中英文 reciprocal link。
- Table、code block、ID name 与 fixture list 语义一致。
- 已从当前目录检查相对源码链接。
- 已检查 Markdown whitespace 与 diff hygiene。
- 上述 targeted formatting、package test、Clippy、rustdoc 与 dependency audit
  已通过。
- 由于该 crate 没有 I/O 或 host behavior，有意跳过 ECS、provider 与 host-mutation
  validation。
