# Phase 0 Storage and Supervision 设计

[English](design.md) | [验收基线](acceptance_zh.md) |
[规划集](../../README_zh.md)

## 状态与决策

- 基线：`up/main` 的 `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`
- 状态：ADR-S1 首个 storage 切片已实现；其余 storage operation 与 ADR-S2 由各自验收证据继续跟踪

本模块做出两项架构决策：

1. **ADR-S1：** Task event、projection、idempotency、approval、permit、execution、
   Runtime binding 与 Outbox delivery 使用 WAL mode 的 embedded SQLite，并由一个
   local application writer 写入。
2. **ADR-S2：** 由一个 Gateway `RuntimeSupervisor` 独占所有 Agent Runtime child
   process，包括 cosh-core 与 ACP Agent。Shell 只保留 interactive PTY process
   ownership。

这些决策不会把 provider conversation persistence、audit segment 或 terminal
evidence 合并进 Task database。

### ADR-S1 实现说明

当前候选新增 `cosh-gateway::storage`，使用 SQLite WAL、`synchronous=FULL`、foreign key、
`trusted_schema=OFF`、五秒 busy timeout 与 strict table。单一 private connection 仅通过
`&mut SqliteTaskStore` 暴露可变访问。它要求 absolute database path，把缺失的专用目录创建为 `0700`、
database 创建为 `0600`，拒绝 relative path、不安全 existing parent、non-regular file 以及任一已存在
path component 中的 symlink，并在 open 前后检查既有 WAL/SHM companion。

首个 schema 原子拥有 Task、Task event、actor-scoped command receipt 与 Outbox intent。Checksummed
migration、newer-schema refusal、`quick_check`、确定性 event recovery 与完整 transaction rollback 已有
automated evidence。Online backup/restore、checkpoint health、disk-full injection、race-free
descriptor-relative open、Outbox lease 与 execution reconciliation 仍是 storage exit 前的必需项。

## 目标

- Atomic commit command deduplication、Task event、projection 与 Outbox work。
- Daemon 或 host restart 后恢复 Task state，不把 Agent process 当成事实来源。
- Fence crashed/replaced Runtime generation 的 output。
- 每个 child process 只有一个 owner，负责 spawn、pipe、cancellation、escalation、
  reap、resource bound 与 diagnostics。
- 迁移期间保留现有 provider-session file 与 audit storage。
- 支持不需要 external database 的 local-first installation。

## 非目标

- Distributed consensus、active-active Gateway replica 或 network-shared SQLite file。
- 在 Task event 中保存 model transcript、raw terminal output、secret 或 provider stderr。
- 保证 arbitrary OS side effect 的 replay safety。Unknown execution outcome 需要
  reconciliation 或 user approval。
- 由 Gateway 监督用户 native Shell job。
- 替换现有 session persistence、audit JSONL 或 ws-ckpt protocol。
- 在 Phase 0-2 采用仍处于草案状态的 ACP Streamable HTTP transport。

## 当前源码证据

| 证据 | 已核实的基线行为 | 目标架构缺口 |
| --- | --- | --- |
| [`SessionStore`](../../../../../crates/cosh-core/src/session/store.rs#L40) | Workspace-scoped provider session 使用 versioned envelope、generation check、lock 与 atomic commit | 它不是 multi-aggregate Task/Event/Outbox transaction store |
| [`ScopedStorage`](../../../../../crates/cosh-core/src/session/scoped.rs#L27) | Descriptor-relative access、`0700` directory、`0600` file、no-follow open 与 atomic rename 强化 session file | Database creation 与 backup path 需要等价保护 |
| [`PersistedSession`](../../../../../crates/cosh-core/src/session.rs#L83) | Provider transcript 与 compaction projection 已持久化 | Provider history 与 Task state 仍是不同 data class |
| [`CoshCoreService`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service.rs#L47) | Shell 拥有一个 persistent Core process、worker thread、cancellation 与 restart/reset decision | Ownership 是 Shell-local，无法承载 detached Web/channel Task |
| [`spawn_provider_child`](../../../../../crates/cosh-shell/src/adapter/process.rs#L66) | Provider child 使用独立 session/process group、bounded stderr、watchdog、TERM/KILL 与 reap | 逻辑分散在 Shell adapter 中，不是 durable Runtime supervisor |
| [`output_with_timeout`](../../../../../crates/cosh-core/src/process.rs#L72) | Core 对 bounded helper subprocess 也有 process-group cleanup | Agent Runtime lifecycle 没有统一 owner |
| [`Cargo.toml`](../../../../../Cargo.toml) | Candidate 已声明 `cosh-gateway` 使用的 workspace SQLite client | focused store slice 已存在；完整 ADR-S1 exit evidence 仍不完整 |
| [Unified audit design](../../../../../docs/design/audit-log.md) | Audit 使用 per-process JSONL segment，拥有独立 durability 与 retention semantics | Audit 不能被静默转入 Task SQLite |

## Data-class 边界

| Data class | Owner 与 store | 分离原因 |
| --- | --- | --- |
| Task lifecycle | Gateway SQLite | Transactional command/event/projection/Outbox invariant |
| Provider conversation | 首版沿用 cosh-core `SessionStore` | Model transcript、workspace resume、compaction 与 provider compatibility |
| Audit | 现有 versioned per-process segment | Append-only operational record，独立 failure policy 与 retention |
| Terminal evidence | Shell/evidence owner | 可能很大、短期存在，并通过 opaque ID 引用 |
| Runtime diagnostics | Supervisor bounded memory 加 redacted audit reference | Stderr 与 protocol failure 不得原样进入 domain event |

后续 consolidation 需要单独 migration ADR。Phase 1 可以从 Runtime binding 引用
provider session，但不复制其中 message。

## ADR-S1：SQLite WAL Task Store

### 决策

首个 Gateway 使用 private local database，解析顺序如下：

```text
$COSH_GATEWAY_STATE_DIR/state.db
$XDG_STATE_HOME/cosh/gateway/state.db
$HOME/.local/state/cosh/gateway/state.db
```

Parent directory 使用 `0700`；database、WAL、shared-memory、backup 与 migration
file 对 effective user 保持 private。Open 拒绝 symlink 与 non-regular file。
Phase 1 只支持 local filesystem。Network/shared filesystem 不受支持，必须在启动
校验时失败，不能静默降级。

每个 connection 使用等价 database policy：

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA trusted_schema = OFF;
```

Task admission、approval、permit、execution 与 Outbox durability 选择
`synchronous = FULL`。若通过测量放宽到 `NORMAL`，必须有 ADR 并记录 data-loss
window。WAL checkpoint policy 必须 bounded 且 observable；checkpoint failure
使 health degraded，但绝不能删除 WAL。

### 单 Writer Model

- 一个 bounded Gateway writer task 独占 write connection。
- 所有 state-changing command 经过 authentication 与 size validation 后进入该 queue。
- Writer 使用 `BEGIN IMMEDIATE` 与短 transaction。
- Read-only projection query 使用 bounded reader connection，不启动 write transaction。
- Bridge、presenter、HTTP handler、Shell attachment 与 executor 都不能持有 database
  connection 或直接写 table。
- Queue saturation 在 admission 前返回 stable overload error；caller 用同一
  idempotency key retry。

Single writer 是 application ownership rule，不表示 SQLite 不能支持多个 writer。
它为 local control plane 明确 ordering、backpressure、migration 与 failure semantics。

### Schema Ownership 草案

```text
schema_migrations(version, checksum, applied_at_ms)
gateway_meta(key, value)
actors(actor_id, issuer, subject_digest, assurance, status, ...)
commands(command_id, actor_id, idempotency_key, payload_digest,
         accepted_at_ms, result_event_id, ...)
tasks(task_id, owner_actor_id, target_ref, revision, state, ...)
task_events(event_id, task_id, revision, event_type, schema_version,
            payload_json, occurred_at_ms, causation_message_id)
runs(run_id, task_id, attempt, state, terminal_event_id, ...)
external_refs(external_ref_id, kind, authority, scope_digest,
              value_ciphertext_or_value, value_digest)
runtime_instances(runtime_instance_id, generation, launch_digest,
                  state, last_exit_code, ...)
runtime_bindings(binding_id, task_id, run_id, runtime_instance_id,
                 runtime_generation, external_ref_id, state, ...)
approvals(approval_id, task_id, run_id, request_digest, state, ...)
permits(permit_id, approval_id, task_id, run_id, target_digest,
        operation_digest, expires_at_ms, consumed_by_execution_id, ...)
executions(execution_id, permit_id, state, idempotency_scope,
           result_digest, evidence_ref, ...)
outbox(delivery_id, event_id, sink_kind, sink_ref_digest, state,
       attempt, next_attempt_at_ms, lease_owner, lease_expires_at_ms, ...)
```

`task_events` 对 `(task_id, revision)` 建立 unique constraint，row immutable。
Projection table 与 event 在同一 transaction 中更新，并可通过 event 与显式 versioned
migration logic 重建。Event payload 使用 frozen contract schema，serialization 前
必须 bounded。

### Transaction Contract

#### Command admission

一个 transaction 完成：

1. Insert 或校验 scoped idempotency row；
2. 读取并比较 Task revision；
3. 追加一个或多个 Task event；
4. 更新 Task/Run/Approval/Execution projection row；
5. 把 presentation 与 Runtime dispatch intent 加入 Outbox；
6. Commit 后唤醒 worker。

Database transaction 内不调用 Runtime，也不执行 OS side effect。

#### Execution boundary

1. Atomic verify/consume single-use Permit，并创建 `Execution(state=starting)`。
2. 在 transaction 外通过 `ExecutionTargetPort` 执行。
3. 在一个 transaction 中持久化 terminal result 与 Task event。
4. 如果 process 或 host 在步骤 1 后 crash，recovery 把 Execution 标记为
   `outcome_unknown`；不能重复 non-proven-idempotent mutation。

#### Outbox delivery

Worker lease bounded batch。Delivered sink acknowledgement atomic mark row delivered。
Lease expiry 允许用同一 `DeliveryId` retry。Consumer 必须按 Delivery ID 去重，
或接受 at-least-once delivery。

### Migration 与 Recovery

- Migration ordered、checksum-pinned，并在 SQLite 允许时保持 transactional；只能由
  writer owner 在开始服务前执行。
- Binary 拒绝更新 schema version 的 database。
- 启动时执行 bounded metadata validation 与 `quick_check`；完整 integrity check
  属于显式 maintenance operation。
- Destructive migration 前，在 private sibling path 创建 verified SQLite online backup，
  并记录 restore procedure。
- Migration failure 使 daemon unavailable；不能通过 partial read-only mode 批准或
  执行 work。
- Restart 时 fence in-flight Runtime binding、reclaim expired Outbox lease，
  uncertain execution 等待 reconciliation。

### 比较过的替代方案

| 替代方案 | 优点 | Phase 1 不选择的原因 |
| --- | --- | --- |
| Append-only JSONL 加 rebuilt projection | 易检查，与 audit segment 一致 | Atomic event/projection/idempotency/Outbox update 与 indexed concurrency 需要大量自研 recovery |
| 每个 Task 一个 atomic JSON file | 可复用 SessionStore pattern | Cross-Task query、Outbox lease、uniqueness 与 multi-entity transaction 会变成复杂 lock choreography |
| 现有 `SessionStore` | Secure file handling 成熟 | Aggregate 是 provider conversation history，不是 Task/Approval/Execution state |
| Embedded KV（`redb`、`sled`、RocksDB） | Key-value access 快 | 对 relational workload 需要自研 schema/index/transaction tooling，operational inspectability 较弱 |
| Rollback-journal SQLite | File set 更简单 | Write 时 reader 更容易 blocked；WAL 更适合 attachment replay 与 dashboard |
| External PostgreSQL | Multi-node operation 强 | 破坏 zero-dependency local-first installation，在 multi-replica 要求出现前没有必要 |
| In-memory state 加 audit replay | 初期实现小 | 丢失 durable idempotency，并把 audit 与 authoritative event store 混淆 |

SQLite WAL 不解决 distributed ownership。如果未来需要 active-active Gateway 或 network
storage，必须通过新的 storage-port implementation 与 data migration ADR 迁移。

## ADR-S2：Runtime Supervisor Ownership

### 决策

`RuntimeSupervisor` 是唯一允许操作 cosh-core 与 ACP Agent child process handle 的
implementation。Bridge 拥有 protocol codec 与 session semantics，也可以像当前
`AcpV1RuntimeBridge` 一样组合且仅组合一个 Supervisor。Spawn、signal、wait 与 reap
仍必须委托给该内嵌 Supervisor，任何第二 lifecycle owner 都不能保留 child handle。

| Process/resource | Sole owner | 说明 |
| --- | --- | --- |
| Native bash/zsh PTY 与 foreground job | `cosh-shell` Shell host | 保留 terminal job control 与 attachment experience |
| cosh-core Agent Runtime | Phase 1 迁移后的 Gateway `RuntimeSupervisor` | 当前 Shell owner 仅在 compatibility fallback 中保留 |
| ACP Agent stdio process | Gateway `RuntimeSupervisor` | Phase 2 仅 local stdio |
| Short-lived Core helper subprocess | 现有 scoped Core owner | 不是 Agent Runtime，继续使用 process-group cleanup |
| OS operation execution | `ExecutionTargetPort` implementation | 受 Permit 治理，不是 Runtime bridge child |

### Typed Supervision Contract

```rust
struct RuntimeLaunchSpec {
    kind: RuntimeKind,
    executable: TrustedExecutable,
    args: Vec<BoundedArg>,
    cwd: CanonicalPath,
    env: AllowlistedEnvironment,
    protocol: RuntimeProtocol,
    resource_profile: ResourceProfile,
    restart_policy: RestartPolicy,
}

struct SupervisedRuntimeRef {
    instance_id: RuntimeInstanceId,
    generation: u64,
    launch_digest: Digest,
}

enum SupervisorCommand {
    EnsureRunning { spec: RuntimeLaunchSpec },
    OpenChannel { runtime: SupervisedRuntimeRef },
    CancelRun { runtime: SupervisedRuntimeRef, run_id: RunId },
    Stop { runtime: SupervisedRuntimeRef, reason: StopReason },
}

enum SupervisorEvent {
    Started { runtime: SupervisedRuntimeRef, pid_observed: u32 },
    Ready { runtime: SupervisedRuntimeRef },
    ProtocolFailed { runtime: SupervisedRuntimeRef, code: RuntimeErrorCode },
    Exited { runtime: SupervisedRuntimeRef, exit: BoundedExit },
    RestartScheduled { previous: SupervisedRuntimeRef, backoff_ms: u64 },
}
```

PID 只用于 diagnostics。`RuntimeInstanceId + generation` 是 fencing identity。
Secret 由 credential provider 引用，只在 child launch environment 中 materialize；
launch digest、event 与 diagnostics 都不包含 secret。

### State Machine

```text
Absent -> Starting -> Initializing -> Ready <-> Busy
             |             |           |        |
             +-------------+-----------+--------+-> Stopping -> Exited
                                      \----------> Failed -> Backoff -> Starting(new generation)
```

- 每次 spawn 在任何 event admission 前增加 generation。
- `Ready` 要求 protocol initialization、version 与 capability validation 通过。
- 只有 negotiated protocol 与 scheduler policy 允许时，bridge 才能 multiplex session。
  一个 ACP connection 可支持多个 session，但 Phase 2 不假定所有 Agent 都能安全并发 prompt。
- cosh-core process reuse 继续遵循当前 approval mode、workspace scope 与
  provider-session binding constraint。
- Unexpected exit 使对应 generation 的所有 binding stale。Task state 保持 durable，
  并决定 resume、retry 或 user intervention。
- Restart budget 使用 bounded exponential backoff 与 circuit-open terminal health state；
  crash loop 不会无限旋转。

### Spawn 与 I/O 安全

- Executable 从 trusted installation/configuration 解析，不从 user prompt text 解析。
  记录 executable/argument digest。
- Spawn 前 canonicalize cwd 并校验 target access。
- Child 使用自己的 process group/session；不把 PID 当作 durable identity。
- Protocol 使用 piped stdin/stdout、持续 bounded stderr drain、最大 line/message size、
  bounded queue 与显式 backpressure。
- Protocol stdout 只能包含 protocol frame。Human log 写 stderr，并在 diagnostic
  retention 前 redacted/bounded。
- 所有无关 descriptor close-on-exec。Child environment 使用 allowlist，默认不继承
  Gateway/channel credential。
- Pipe 与 process-group setup 成功后才注册 child ownership；partial-spawn failure
  也必须 kill/reap。

### Cancellation 与 Shutdown

对于 active Run：

1. 在 Task store 持久化 `CancelRequested`；
2. Bridge 在可用时发送 protocol cancellation。ACP prompt 使用 `session/cancel`，
   cosh-core 使用当前 interrupt；
3. 在 bounded protocol grace 内接受允许的 terminal update；
4. Retire connection 时关闭 stdin 或发送 shutdown；
5. 向 process group 发送 `SIGTERM`；
6. Bounded grace 后向 process group 与 direct child 发送 `SIGKILL`；
7. Reap child 与 reader task；
8. 用同一 Runtime generation 持久化 observed cancellation/exit outcome。

Daemon shutdown 停止 admission、持久化 pending cancellation/handoff state、在 deadline
内 drain Outbox、终止 Runtime child，最后关闭 SQLite。Shell PTY shutdown 继续由 Shell
拥有。

### Restart 与 Orphan Policy

- Gateway child process 不会在 daemon exit 时故意 orphan。
- Daemon restart 时 durable Runtime instance 变为 `stale`；Supervisor 不会仅根据 PID
  number attach process。
- 未来 Runtime process detach/reattach 需要 brokered socket、authenticated ownership
  token 与单独 ADR。
- 无副作用 Task 可根据 bridge capability resume/retry。处于 `starting` 或 `running`
  的 Execution 必须经过 target-specific reconciliation 才能 retry。

## Error 与安全边界

- Storage unavailable、migration failure、critical row corrupt 或 schema mismatch 会阻止
  新 governed execution。
- 只有在不修改 lease 或 acknowledgement 时，read-only UI 才能暴露显式 degraded state。
- Database error 使用 stable safe code；不向渠道返回 SQL、含 private data 的 path、
  payload JSON 或 secret。
- Supervisor error event 只包含 bounded exit class 与 redacted stderr reference，不包含
  raw stream。
- Bridge 不能通过 terminal/filesystem callback 绕过 Broker authorization。
- 每个 event 与 permission response 都校验 Runtime generation 与 launch digest。
- Database backup/export 需要显式 authorization 与 private output permission。

## 兼容与迁移

1. 在新 port 后添加 SQLite store 与 Runtime Supervisor，不改变当前 Shell behavior。
2. 在 Supervisor control 下实现 CoshCore Bridge；把当前 Shell-local service 保留为
   feature-gated fallback。
3. 持久化 Task 到 existing-provider-session binding，不把 transcript message 导入 SQLite。
4. Phase 1 storage/restart gate 通过后，才把 Shell 切换为 Gateway attachment。
5. Phase 2 在同一 Supervisor 下添加 ACP Runtime。
6. 兼容窗口后移除重复的 Shell Agent process ownership；Shell 继续拥有 native PTY。

最终 cutover 前的 rollback 会关闭 Gateway admission 并恢复当前 Shell-local path。
Database file 保留用于 forward recovery；rollback code 不得 downgrade 或重写更新 schema。

## 依赖

- [Protocol Contracts](../protocol-contracts/design_zh.md)定义 stored event 与 supervisor
  port payload。
- [Identity and Correlation](../identity-correlation/design_zh.md)定义 foreign-key identity
  与 generation fencing。
- Phase 1 Task Plane 实现 writer 与 reducer。
- Phase 1 CoshCore Bridge 是首个 supervised Runtime。
- Phase 2 ACP bridge 消费 supervised stdio。

## 实施任务

1. 记录 ADR-S1 与 ADR-S2 的 acceptance，包括 local-filesystem support。
2. 按 workspace dependency policy 选择 maintained SQLite Rust crate。
3. 实现 secure state-path creation、connection policy、migration、writer queue、reader、
   health、backup 与 restore tooling。
4. 实现 schema 1、atomic command/event/projection/Outbox transaction 与 crash recovery。
5. 实现 supervisor state machine、process group、bounded I/O、generation fencing、
   restart budget 与 shutdown ordering。
6. 把 CoshCore Bridge process ownership 移到 Supervisor 后。
7. 添加 fake Core 与 ACP child fixture，覆盖 crash、hang、malformed output、
   cancellation 与 process-tree leakage。
8. 默认启用 Gateway 前，记录 operational status、backup、restore、corruption 与
   disk-full procedure。

## 测试策略

- SQLite test 覆盖 transaction rollback、unique revision、foreign key、idempotency
  conflict、Outbox lease、migration checksum、disk full、checkpoint failure、
  corruption、backup 与 restore。
- Crash fixture 在每个 transaction boundary 后停止 process，并验证 replay/
  reconciliation behavior。
- Concurrency test 饱和 writer/reader queue，不绕过 sole writer。
- Supervisor test 覆盖 partial spawn、invalid initialization、huge line、closed pipe、
  stderr flood、timeout、忽略 TERM 的 child、grandchild、crash loop、shutdown 与
  stale-generation output。
- Test 不执行 privileged OS mutation。Process test 使用 deterministic fixture program
  与 temporary directory。

## 开放决策

| 决策 | Owner | 最晚关闭时间 |
| --- | --- | --- |
| SQLite Rust crate 与 feature set | Storage owner | 第一个 Phase 1 storage PR |
| WAL auto-checkpoint 与 maximum WAL health threshold | Storage/SRE owner | Restart acceptance 前 |
| Raw external reference value 的 encryption mechanism | Security owner | Schema migration 1 freeze 前 |
| 每种 Agent implementation 的 Runtime pool concurrency | Runtime owner | Bridge-specific acceptance 前 |
| Linux pidfd/subreaper 或 process-group baseline | Runtime owner | Supervisor implementation review 前 |
| Task event database retention/compaction policy | Product 与 storage owner | Public Gateway rollout 前 |
