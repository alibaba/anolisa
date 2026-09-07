# PAP daemon API V2 工作包验收记录

| 属性 | 值 |
| --- | --- |
| 状态 | PAP integration ready；不是 distribution/release ready |
| 验收日期 | 2026-09-07 |
| 源码基线 | `main@9f109d55964cfc5820d41564869f70879a8de21f` |
| contract revision | 本文、`DAEMON_PROTOCOL_V1_zh.md` 和 `DAEMON_PROCESS_DEPLOYMENT_CONTRACT_zh.md` 同一变更 |

## 1. Goal、范围和非目标

本工作包为 V2 daemon 增加 15 个显式 allowlisted PAP 方法，覆盖 Policy、Scope 和 Binding
的 current-record CRUD；由 kernel peer credentials 构造 trusted Principal，经
`asc-daemon-handler` 调用 `asc-daemon-core::PolicyAdministration`，再委托给 `PapService`。
工作包同时提供首个 `prevent_file_deletion` compiler 和仅用于集成的 process-local
Repository。

以下不属于本工作包的完成声明：durable persistence、Reconciler、target Adapter、真实
AgentSight/ActPlane enforcement、production socket/package hardening、完整 readiness，以及
V1 九个 daemon method 的替代或退役。

## 2. Crate relationship 与 acceptance type

| crate / entrypoint | V1 relationship | acceptance type | 当前证据层级 |
| --- | --- | --- | --- |
| `asc-policy-types` authored contract correction | greenfield V2 contract | `GREENFIELD_CONTRACT` | crate contract |
| `asc-policy-engine` | greenfield | `GREENFIELD_CONTRACT` | direct trait + daemon consumer |
| `asc-pap-repository-memory` | adapter；无 V1 durable-state 承诺 | `ADAPTER_CONFORMANCE` | Repository + PAP consumer |
| `asc-daemon-protocol` PAP method family | greenfield method set over an existing V1 envelope | `GREENFIELD_CONTRACT` | typed wire + real UDS |
| `asc-daemon-core` PAP boundary | greenfield | `GREENFIELD_CONTRACT` | handler consumer |
| `asc-daemon-handler` | protocol/application adapter | `ADAPTER_CONFORMANCE` | real UDS |
| `asc-daemon` composition | partial migration | `PARTIAL_EQUIVALENCE` | foreground binary + UDS + signal cleanup |

直接依赖 contract 使用本基线中的 `asc-foundation-types`、`asc-policy-types`、`asc-pap` 和
`asc-daemon-service`。V1 Python daemon 只作为 discovery/oracle 来源，不进入 Rust runtime。

## 3. External compatibility report

- 15 个 `policy.*` 方法是 `[TARGET V2]` 新增面，不冒充当前 V1 九个 method，也不修改现有
  CLI、Python daemon 或 V1 action response。
- PAP wire 直接复用 domain `PolicyTemplate`、`ScopeSelector`、`PreparedPolicy`、
  `PreparedScope` 和 `BindingView`；method params 拒绝未知字段。
- `prevent_file_deletion` 的 contract 明确收敛为
  `ResourceOperation::Delete + FileResolution::PathEntry`。它不承诺阻止 rename/move、link、
  truncate、内容修改或其它 namespace mutation。
- daemon 为每个 dispatch 生成新的 UUID request ID；成功和失败 response 均只暴露有界的
  public error contract，不暴露内部 persistence/compiler error。
- shared V1 request envelope 的 `trace_context`、`caller`、`timeout_ms` 和未知顶层字段兼容性
  尚未在此工作包中实现，因此本记录不声明 V1 envelope compatibility 已完成。

## 4. Internal contract change record

| ID | 变更 | 原因与影响 |
| --- | --- | --- |
| PAP-CR-001 | 将 `PreventFileDeletion` 从含糊的 rename-out 表述收窄为 path-entry delete | 当前 compiler、protocol fixture 和 IR 只生成 `Delete`；在进入 distribution 前消除过度承诺，不改变 V1 runtime |
| PAP-CR-002 | `DaemonError` 在构造和 decode 时统一限制为 256 UTF-8 bytes | 防止任一 handler 绕过公共 response bound；超长 caller-authored decode error 使用稳定通用消息，不回显输入 |
| PAP-CR-003 | `pid`、`cgroupId` selector validation path 投影为 `InvalidArgument` | 这些路径来自 authored selector，不是 canonical/internal state |
| PAP-CR-004 | serialized CRUD scenario 要求所有 dispatch request ID 互不相同 | 冻结每个请求生成 fresh UUID 的 correlation contract |

若未来需要阻止 rename-out，必须先为 source/destination namespace 语义建立 IR 和直接 Adapter
conformance；不能只把所有 `NamespaceMutation` 无差别加入当前 rule。

## 5. Pass/fail matrix

| ID | 验收项 | executable evidence | 结果 |
| --- | --- | --- | --- |
| PAPAPI-001 | compiler 输入、完整 IR 和 delete-only 语义 | `asc-policy-engine/tests/compiler_contract.rs` + `compiler-contract.json` | PASS |
| PAPAPI-002 | process-local Repository 满足当前 PAP request slice | `asc-pap-repository-memory` unit tests | PASS |
| PAPAPI-003 | 15 个 method 的完整 serialized CRUD | `asc-daemon/tests/pap_protocol.rs::real_uds_executes_the_complete_pap_crud_fixture` | PASS |
| PAPAPI-004 | invalid params、domain validation、not-found 与稳定 error | `real_uds_rejects_every_invalid_crud_parameter_class` | PASS |
| PAPAPI-005 | server-owned authorization 且 caller data 不提权 | handler/core tests、all-method deny UDS test、binary DPROC test | PASS |
| PAPAPI-006 | 每个 daemon dispatch 返回有效且唯一的 UUID | shared serialized CRUD runner | PASS |
| PAPAPI-007 | process bootstrap、signal 和 socket cleanup | `dproc_002_003_and_partial_013_binary_registers_pap_and_cleans_socket` | PASS（DPROC-013 partial） |
| PAPAPI-008 | full workspace regression | `cargo test --workspace --locked` | PASS |
| PAPAPI-009 | lint、format 和 API docs | Clippy、rustfmt、Rustdoc commands below | PASS |
| PAPAPI-010 | durable state、target enforcement 和 packaging rollout | 不在本工作包范围 | NOT RUN |

可重复执行命令：

```bash
cd v2
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
cargo doc --workspace --no-deps --locked
git diff --check
```

## 6. Direct-consumer evidence 与限制

- `PolicyTemplateCompiler` 通过 `PolicyCompiler` port 被 `PapService` 调用，并由完整 UDS CRUD
  scenario 消费。
- `ProcessLocalPapRepository` 通过 `PapRepository` port 被同一 `PapService` 消费；并发/CAS
  行为由 PAP 和 Repository tests 覆盖。
- `PolicyAdministration` 被 `DaemonDispatcher` 消费；serialized bytes 经真实 UDS，而不是只调用
  Rust struct constructor。
- binary fixture 只能证明当前 host 身份、protocol 注册、permission 和 signal cleanup；不能
  替代安装后 systemd/container/Kubernetes 或真实 target enforcement。
- memory Repository 在进程重启后丢失全部状态，不得作为 durable acceptance evidence。

## 7. Rollback

回滚本工作包时撤销对应 Rust commit，并从 workspace/composition root 移除新增 crate 和 PAP
dispatcher registration。当前 Repository 不写 durable state，因此没有 schema/state downgrade；
停止进程后其状态即消失。若只回滚 contract correction，不得仅恢复 rename-out 文案：必须连同
支持该语义的 IR、compiler、Adapter fixture 和版本化兼容记录一起交付。
