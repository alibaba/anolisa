# Phase 0 Storage and Supervision 验收基线

[English](acceptance.md) | [设计](design_zh.md) |
[规划集](../../README_zh.md)

## 基线结论

**ADR 方向已用于规划；实现 readiness 未通过。** 已审计源码为
`6c115aefe04ace0d169a24fa7cd55ad7c1befa52`。

基线已经有安全的 provider-session file persistence 与多个成熟的 process-tree
cleanup path，但没有 SQLite dependency、Gateway Task store、Outbox、Runtime
Supervisor、daemon recovery 或 generation fencing。

## 首个 ADR-S1 实现结果

**Storage 结果：首个切片已验证；Storage Exit 尚未接受。** 当前工作树候选已实现 Task transaction 与
local SQLite connection policy。Runtime supervision 单独验收，其最终状态由 root integration report 负责。

2026-08-13 记录：

- `cargo test --locked --package cosh-gateway storage --no-fail-fast` 通过 14/14。
- `cargo test --locked --package cosh-gateway task::aggregate --no-fail-fast` 通过 6/6。
- `cargo clippy --locked --package cosh-gateway --lib -- -D warnings` 通过。
- Automated evidence 覆盖 WAL/FULL/foreign-key policy、actor 与 revision substitution、Task/Event/receipt/
  Outbox atomic rollback、checksummed/newer-schema failure、确定性 reopen recovery、causation row、relative
  path、不会 chmod 的 insecure parent，以及 intermediate/final symlink。

结果口径中，`PASS` 表示完整可复现证据；`PARTIAL` 表示已有验证切片但仍有明确缺口；
`NOT IMPLEMENTED` 或“缺失”表示没有 production path；`BLOCKED` 表示指定依赖阻止验证。

## 已审计证据

| 来源/符号 | 已核实事实 |
| --- | --- |
| [`SessionStore::persist`](../../../../../crates/cosh-core/src/session/store.rs#L125) | 对单个 provider-session aggregate 使用 validation、lock、generation conflict detection、redaction、bound 与 atomic file commit |
| [`ScopedStorage`](../../../../../crates/cosh-core/src/session/scoped.rs#L27) | 使用 private permission、descriptor-relative operation、no-follow open 与 temporary-file cleanup |
| [`CoshCoreService::new`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service.rs#L106) | Shell 启动拥有 persistent cosh-core process state 的 worker |
| [`service_loop`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service.rs#L283) | Shell 根据 per-turn state reset 或 shutdown Core child |
| [`spawn_provider_child`](../../../../../crates/cosh-shell/src/adapter/process.rs#L66) | Provider process 使用 new session、piped I/O 与 bounded retry |
| [`run_provider_process_loop`](../../../../../crates/cosh-shell/src/adapter/process.rs#L190) | Shell 已有 watchdog、bounded stderr、cancellation escalation 与 reap |
| [`output_with_timeout`](../../../../../crates/cosh-core/src/process.rs#L72) | Core helper subprocess cleanup 覆盖 timeout 与 caller cancellation |
| [`Cargo.toml`](../../../../../Cargo.toml) | 没有声明 SQLite dependency |

没有运行 provider、ECS、privileged 或 live process test。上述命令是首个实现切片的 local targeted test；
历史基线本身仍是 documentation evidence。

## 验收矩阵

| ID | 要求 | 基线 | 通过所需证据 |
| --- | --- | --- | --- |
| SS-01 | ADR-S1 明确接受 SQLite WAL、single writer 与 local filesystem only | PASS | Connection-policy 与 private-path test。 |
| SS-02 | Task event、projection、idempotency 与 Outbox atomic commit | PASS | 重复 Delivery ID 使 projection/event/receipt transaction 完整 rollback。 |
| SS-03 | Schema migration checksummed、fail closed、可 backup/restore | PARTIAL | Checksum/newer-schema/quick-check 已通过；online backup 与 restore fixture 待补。 |
| SS-04 | Private path、no-follow、ownership 与 file-type check 保护所有 SQLite companion file | PARTIAL | Absolute/private/path-component test 已通过；race-free descriptor-relative open 与 ownership check 待补。 |
| SS-05 | Event revision 与 identity parent 由 database constraint 执行 | PARTIAL | Strict DDL 强制 event ID、`(task_id, revision)` 与已有 foreign key；并非每类 parent 都已有 DB row。 |
| SS-06 | Unknown execution outcome 不会 auto-replay unsafe side effect | 缺失 | Crash-boundary reconciliation test |
| SS-07 | ADR-S2 把全部 Agent child ownership 交给一个 `RuntimeSupervisor` | PARTIAL | Supervisor 首个切片与 owned test 已单独验证；daemon ownership migration 待补。 |
| SS-08 | 迁移后 Shell 只拥有 native PTY；bridge 不拥有 process handle | 缺失 | Ownership inventory 与 compile/API review |
| SS-09 | 每次 spawn 都有 process-group cleanup、bounded I/O、reap 与 generation fencing | PARTIAL | Supervisor process cleanup 与 bounded I/O 已单独测试；generation fencing 待补。 |
| SS-10 | Restart backoff 与 circuit-open health 防止 crash loop | 缺失 | Deterministic clock/restart-budget test |
| SS-11 | Daemon restart fence binding、reclaim lease 并 reconcile execution | 缺失 | End-to-end restart fixture |
| SS-12 | Session、audit、evidence 与 Task store 保持分离 | PASS | 新 Gateway schema 不替换 SessionStore/audit/evidence。 |
| SS-13 | 双语文档、链接与命令等价 | PASS | Reciprocal link 与 implementation evidence 已镜像。 |

`PARTIAL` 表示已经验证一个切片，不表示完整 supervisor 或 storage exit。

## 必要 Fixture 与 Artifact

```text
fixtures/gateway-storage/v1/
  schema.sql
  migrations/
    0001_initial.sql
  task-command-atomicity.json
  outbox-reclaim.json
  execution-outcome-unknown.json
  migration-checksums.json
  corrupt/
    newer-schema.db
    invalid-foreign-key.db
    truncated-wal.db
fixtures/runtime-supervisor/v1/
  fake-core-normal
  fake-acp-normal
  malformed-initialize
  oversized-line
  stderr-flood
  close-stdout
  ignore-term
  spawn-grandchild
  crash-loop
```

必要 operational artifact：

- Accepted ADR-S1 与 ADR-S2；
- Schema diagram 与 migration compatibility table；
- State-path 与 file-permission specification；
- 包含验证结果的 backup/restore runbook；
- Disk-full、corruption、stuck WAL 与 crash-loop runbook；
- 证明每个 child 只有一个 owner 的 process ownership inventory；
- Deterministic fixture 产生的 supervisor transition 与 shutdown trace。

基线中不存在这些 artifact。

## 必要验证命令

最终 package 名可遵循 implementation scaffold，但验收必须记录下列等价 targeted
command 与准确 count：

```bash
cargo test --package cosh-gateway storage
cargo test --package cosh-gateway --test storage_faults
cargo test --package cosh-gateway runtime_supervisor
cargo test --package cosh-gateway --test supervisor_process_tree -- --test-threads=1
cargo test --package cosh-shell --test protocol
cargo test --package cosh-shell --test shell_host -- --test-threads=4
```

Shell target 验证迁移没有破坏当前 protocol 与 PTY ownership。它们是未来实现 gate，
不是本次文档变更已运行的命令。

## 必测 Failure Scenario

| 场景 | 必需结果 |
| --- | --- |
| Task transaction commit 前 crash | 不存在 event、projection 或 Outbox partial state |
| Commit 后、dispatch 前 crash | Outbox 用同一 Delivery ID replay |
| Permit consume 后、result 前 crash | Execution 变为 `outcome_unknown`，不自动重复 unsafe execution |
| Database schema 比 binary 更新 | Startup 在不 mutation 的前提下失败 |
| Migration checksum mismatch | Startup 失败并保留 backup/source database |
| WAL 或 disk full | Admission 停止并显示 stable degraded health，不产生 false success |
| Runtime replacement 后继续输出 | Generation fence 拒绝 Task mutation |
| Child 忽略 protocol cancel 与 TERM | Process group 收到 KILL，所有 descendant 被 reaped |
| Child flooding stderr 或 huge frame | Memory 保持 bounded；Runtime 用 safe code fail |
| Daemon 在 active Task 中 shutdown | Durable state 可解释；没有 orphan Agent Runtime child |

## 剩余实现项

- 没有 online backup/restore、checkpoint/disk health、corruption quarantine 或 operator procedure。
- 没有 Outbox lease/dispatch/ack loop、Run lease 或 uncertain execution reconciliation。
- 当前 path check 会 fail closed，但尚未做到 descriptor-relative 与跨 open 的 race-free。
- `RuntimeSupervisor` 首个切片已有 18 个 owned test；Gateway daemon integration、restart policy 与
  generation fencing 待补。
- Interactive 使用中的 cosh-core 与 provider child ownership 仍然在 Shell 内。
- Library test 可以通过内嵌唯一 `RuntimeSupervisor` 的 `AcpV1RuntimeBridge` 启动 fake ACP
  child；仍没有已安装 entrypoint、live adapter 证据、restart ownership 或 daemon integration。

## Exit Criteria

G0/实现验收要求：

1. SS-01 至 SS-13 在一个准确记录的 commit 上通过。
2. ADR-S1/S2 与其余影响 schema 的决策已批准。
3. 每个 mandatory failure scenario 都有 automated evidence。
4. Backup restoration 针对准确 migration set 测试通过。
5. Process-tree test 证明 cancellation/shutdown 后不泄漏 direct child、grandchild、
   reader 或 writer task。
6. Restart recovery 得到确定性的 Task、Outbox、Runtime binding 与 uncertain
   Execution state。
7. 现有 SessionStore 与 audit fixture 保持通过且不迁移。
8. 除非另行明确请求和记录，不宣称 privileged OS mutation、real provider 或 ECS 结果。

## 验证记录

- 已提供中英文 reciprocal link。
- ADR decision、schema draft、failure matrix、command 与 fixture 在两种语言中一致。
- Relative link 从当前 module directory 可解析。
- 已检查 Markdown whitespace 与 diff hygiene。
- 上文记录了 targeted Storage、Task 与 Runtime test；本次没有运行 full workspace、
  live-system、provider、privileged 或 ECS validation。
