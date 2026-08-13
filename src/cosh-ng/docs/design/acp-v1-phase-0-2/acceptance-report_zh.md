# 总体验收报告

[English](acceptance-report.md)

## 报告身份

| 字段 | 值 |
| --- | --- |
| 基线 | `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`（2026-08-12 获取的 `up/main`） |
| 候选 | 基于该基线的未提交共享工作树；尚无独立候选 SHA |
| 范围 | Phase 0、Phase 1 与 Phase 2 架构就绪度 |
| 被评估的代码变更 | Contracts、Task reducer/SQLite store、Runtime/private JSONL、ACP v1 Bridge/profile 切片与局部 Capability slice |
| 总体实现状态 | **NOT ACCEPTED** |
| 文档集成状态 | **PASS**（通过下述检查；不等于阶段 Gate） |

## 状态词表

| 状态 | 含义 |
| --- | --- |
| `PASS` | 候选 commit 的证据满足验收项 |
| `PARTIAL` | 已有有界源码/测试切片，但模块 exit criteria 或集成路径仍不完整 |
| `FAIL` | 已实现行为经过验证并违反验收项 |
| `NOT IMPLEMENTED` | 被评估 commit 不存在所需生产接口 |
| `BLOCKED` | 接口存在，但环境或前置决策阻止有效验证 |
| `NOT RUN` | 验证适用，但没有被请求或执行 |

`NOT IMPLEMENTED` 不能弱化成 `BLOCKED`。完成设计不构成 Runtime 证据，`PARTIAL` library slice
也不是 production capability。

## 基线结论

基线已经提供以下可复用基础：

- 五个 Rust crate 和显式依赖方向；
- 拥有 PTY 与 Agent 子进程生命周期的独立 `cosh-shell`；
- 精确版本协商的 cosh-core 内部 JSONL 初始化契约；
- 流式 Agent event、approval、question、cancellation、session recovery、audit identity
  与有界 evidence 模式；
- 按 workspace 保存模型 conversation；
- 类型化 package、service、checkpoint 与 audit 操作。

基线源码和 workspace manifest 不包含生产 Gateway daemon、Task aggregate/store/event
store、execution lease、Outbox、Capability Broker、ACP Client dependency 或实现、Web
Attachment API 或 Channel Adapter。因此所有 Phase 1 与 Phase 2 产品 Gate 的初始状态
都是 `NOT IMPLEMENTED`，即使已有组件可以改造成基础实现。

## 候选工作树结论

当前工作树增加了固定基线中不存在的实现基础：

| 切片 | 已实现证据 | 通过验收仍缺少 |
| --- | --- | --- |
| 中立 contract 与 identity | 无副作用的 `cosh-gateway-contracts`，包含 versioned header、有界 leaf string/digest/error、不同 ID newtype、Task/Runtime event、Capability/Approval/Permit shape 与 serde validation | Aggregate collection/envelope admission limit、canonical schema/golden corpus、完整 compatibility manifest、ownership ADR 验收、authenticated identity resolver 与 durable parent/fence enforcement |
| Task reducer | `TaskAggregate` 校验 schema/correlation、连续 revision、active Run、approval/execution lifecycle、cancel、retry 与 terminal transition，失败时不产生 partial mutation | `TaskCoordinator`、command routing、runner lease、durable input、event-admission fencing、完整 transition/property/race suite 与 restart orchestration |
| SQLite Task store | Local single-writer SQLite schema v1，包含 WAL/FULL policy、checked migration、Task projection/event/receipt/Outbox transaction、revision/idempotency check 与 path/symlink/permission validation | Backup/restore、migration artifact、disk-full/corruption/kill-point suite、Outbox worker/lease recovery、daemon restart reconciliation 与完整 companion-file race hardening |
| Runtime 与 private core transport | `RuntimeSupervisor` 校验 direct launch、清除 inherited environment、限制 stdout/stderr、独占 process-group shutdown/reap 并产生一次 process terminal；private COSH JSONL codec 协商 exact v1 并生成 typed local observation | 集成 `CoshCoreBridge`、public contract event mapping/fencing/backpressure、restart/deadline policy、provider-session binding、完整 descendant/race fixture 与 Shell ownership migration |
| ACP v1 第一轮切片 | 准确固定官方 SDK 2.0.0 与 Rust 1.88；codec/Bridge 协商 wire v1；有界 Driver 提供独立 cancel；内置 profile 以 canonical path 与 environment allowlist 解析已安装 Adapter | 已安装 COSH entrypoint、local permission UI/evidence、durable governance/mapping、restart/resume、更广 conformance 与 real-adapter 证据 |
| Capability | 中立 contract、package-exposed Broker 与 mutex-atomic in-memory single-use permit store 已存在；targeted test 覆盖 parent、expiry、target、operation、policy、execution binding 与 concurrent claim | Durable/audited permit ledger、immutable target resolver、execution target/verifier、reconciliation 与关闭 legacy CLI/core/Shell bypass |

候选代码没有实现 Gateway daemon、authenticated Unix/network API、已安装 ACP entrypoint、
Shell Attachment、Web/API Presentation 或 Channel Adapter。Private COSH JSONL v1
codec 与新增 ACP v1 Bridge 继续保持独立。

## 模块就绪度摘要

每个模块的详细报告是该模块的权威记录。

| 阶段 | 模块 | 候选就绪度 | 报告 |
| --- | --- | --- | --- |
| 0 | Protocol Contracts | `PARTIAL`；typed leaf contract 通过 targeted check，frozen schema/fixture 与完整 port 仍缺 | [报告](phase-0/protocol-contracts/acceptance_zh.md) |
| 0 | Identity and Correlation | `PARTIAL`；已有独立 ID/binding，authenticated/durable mapping 与 fence 仍缺 | [报告](phase-0/identity-correlation/acceptance_zh.md) |
| 0 | Storage and Supervision | `PARTIAL`；SQLite/store 与 supervisor 基础存在，recovery、fencing、process-tree 与 ownership migration 仍缺 | [报告](phase-0/storage-supervision/acceptance_zh.md) |
| 1 | Gateway API | `NOT IMPLEMENTED` | [报告](phase-1/gateway-api/acceptance_zh.md) |
| 1 | Task Execution Plane | `PARTIAL`；已有 reducer 与 atomic local store，coordinator/lease/restart path 仍缺 | [报告](phase-1/task-execution-plane/acceptance_zh.md) |
| 1 | Capability Broker | `PARTIAL`；package-exposed in-memory slice 通过 targeted test，但还不是通用 production gate | [报告](phase-1/capability-broker/acceptance_zh.md) |
| 1 | CoshCore Bridge | `PARTIAL`；已有 supervisor/private codec，但 Bridge/public mapping 不存在 | [报告](phase-1/cosh-core-bridge/acceptance_zh.md) |
| 1 | Local ACP Runtime MVP | `PARTIAL`；codec、Bridge、有界 Driver、独立 cancel、fake-Agent test 与固定 profile 已存在，但 installed entrypoint、permission UI/evidence 与 real-adapter proof 不存在 | [报告](phase-1/acp-mvp/acceptance_zh.md) |
| 2 | ACP Client Bridge | `PARTIAL`；官方 v1 codec 与 supervised stdio 切片通过 focused test，domain/governance/recovery integration 仍缺 | [报告](phase-2/acp-client-bridge/acceptance_zh.md) |
| 2 | Shell Attachment | `NOT IMPLEMENTED`；当前存在 direct Shell mode | [报告](phase-2/shell-attachment/acceptance_zh.md) |
| 2 | Web and Presentation | `NOT IMPLEMENTED` | [报告](phase-2/web-presentation/acceptance_zh.md) |

## 阶段 Gate 报告

### G0：Contract Freeze

当前状态：**NOT ACCEPTED**。

退出 Gate 必须满足：

- Ingress、Identity、Task command/event、Approval、Capability、Permit、Execution、
  Runtime event、Presentation、Delivery 和 Error envelope 的 v1 canonical schema；
- 带 backward/forward compatibility 测试的 machine-readable fixture；
- 明确 ID generation、authority、correlation 和 redaction invariant；
- 通过评审的 persistence ADR、migration policy 与 backup/recovery contract；
- 通过评审的 process supervision ADR，每个子进程只有一个 owner；
- ACP v1 feasibility fixture 证明 SDK 与 wire version 分离，分别记录官方 SDK
  2.0.0、Rust 1.88 和稳定 wire v1；
- Dependency 与 crate ownership 决策，保持现有 Shell 边界，或明确记录有意替换。

G0 前，任何 Phase 1 生产 API 都不能冻结自己重复的 contract。

候选 type、SQLite schema、supervision primitive 与 ACP feasibility slice 降低了 G0 实现风险，
但缺少 canonical fixture、ADR sign-off、identity admission 与 recovery artifact，因此 G0 仍未通过。

### G1：Local Durable Gateway

当前状态：**NOT ACCEPTED；只有局部 library foundation**。

退出 Gate 必须满足：

- 本地认证 Unix socket API 与幂等 Task submission；
- 跨进程重启的持久 Task command/event/snapshot 行为；
- Task event 与 Outbox 原子 append；
- 可续租 runner lease 与显式 uncertain-side-effect 处理；
- 通用 Capability Broker，签发绑定 target、会过期且只允许单一 operation 的 permit；
- 通过 platform operator 确定性执行 typed operation；
- cosh-core lifecycle 只能通过 `AgentRuntimePort` 访问；
- cancellation、approval race、crash recovery 与 audit correlation 测试；
- handler、presenter 或 Agent bridge 都不能直接执行 OS action。

Reducer/store/supervisor slice 不提供 daemon、API、TaskCoordinator、runner lease/recovery loop、集成
Runtime Port 或通用 production Capability gate，因此不能满足 G1。

### GM：Local ACP Runtime MVP

当前状态：**NOT ACCEPTED；只有局部 library foundation**。

退出要求一个已安装 COSH entrypoint 通过已安装的 `codex-acp` 或 `claude-agent-acp`，运行且仅运行
一个 canonical workspace、ACP connection/session 与 active bounded text prompt。Session Driver 必须在
stdout 静默或 reader 阻塞时保持 cancel 独立；transport failure 必须 fail closed；local Permission
Proxy 只允许有关联的 `allow_once` 与 `reject_once` decision。至少一个真实 adapter 必须在同一个
candidate revision 上通过 initialize、multi-chunk prompt、terminal result、独立 cancel、allow once
与 reject once。

Codex/Claude 原生 ACP、`npx` 或其他 package runner、network bootstrap、filesystem/terminal callback、
load/resume、Web 与 Gateway daemon 都不属于本 MVP，也不能用来满足它。

### G2：ACP 与 Interactive Attachment

当前状态：**NOT ACCEPTED；只有第一轮 ACP library slice**。

退出 Gate 必须满足：

- 通过本地 stdio 完成 ACP v1 initialization 与 capability negotiation；
- 把 ACP baseline session 与 streaming 行为映射为 Runtime type；
- ACP permission、filesystem 和 terminal request 进入持久 approval 与 Capability Broker；
- incompatible protocol、missing capability、malformed stdout、child exit、cancellation
  与 session recovery conformance case；
- Shell attach/detach/replay，同时保持 PTY ownership 与 direct mode；
- Web/API cursored replay、approval、cancellation 与安全 output view；
- Outbox retry 与稳定 Delivery Receipt 语义；
- 证明 Task、Run、ACP session、Shell session、Request、Tool 与 Execution identity 各自独立。

## 实现验收必须提供的证据包

每个模块实现报告必须包括：

1. 候选 branch 和完整 commit SHA；
2. 被评审 requirement row 与源码链接；
3. 精确 command、environment、test count 与结果；
4. 有版本 fixture 或已脱敏的 protocol transcript；
5. Negative、race 与 failure case，不能只有成功路径；
6. 未验证 provider、ECS、platform 或手工 UI 路径；
7. Rollback 或 compatibility 结果；
8. Security 或 wire-contract 决策的 reviewer sign-off。

证据不能包含凭证、原始 prompt、私有 Terminal output、host identifier 或不受限环境值。

## 跨模块验收场景

这些场景不能由单个 unit test 关闭。

| 场景 | 预期证据 |
| --- | --- |
| 重复钉钉/Web/CLI submission | 只产生一次 Task 状态效果并返回同一个 `TaskId` |
| Gateway 在 event commit 后崩溃 | 恢复 Task 与 Outbox，不重复副作用 |
| OS write 期间 runner lease 过期 | Execution 进入 uncertain 或 reconciliation，不能盲目 replay |
| 两个 Approval callback 竞争 | 一个 terminal decision 生效，两方都取得已提交状态 |
| cosh-core 在 turn 中退出 | 只产生一个 terminal Runtime event，并确定性 suspend/fail Task |
| ACP Agent 请求 Terminal execution | Broker decision 与 permit 先于 target execution，完整 ID 进入 audit |
| Shell 在 Approval 期间 detach | Task 继续 waiting，另一授权客户端无需拥有 PTY 即可处理审批 |
| Web delivery 不可用 | Task 按状态继续，Outbox 独立 retry delivery |
| Provider 网络不可用 | 显式 suspend 或按配置切换端侧模型，不降低 policy |
| 活跃 Attachment 期间 Gateway 重启 | Client 从 cursor replay，不把内存 UI state 当作持久事实 |

## Scope-proportional 候选验证

实现 owner 与集成 owner 对共享工作树中的 Rust slice 运行 targeted package check，文档集成同时
运行对应的双语与仓库文档检查：

- 检查双语文件配对与语义一致；
- 验证相对 Markdown link；
- 运行 `git diff --check`；
- 检查 command 与实现声明是否符合基线和候选源码；
- 保留精确 command 与结果，不把 package evidence 提升为 full-system gate。

不宣称 full workspace test、workspace-wide Clippy、release build、ECS、provider 或手工
Terminal gate。

### 已记录的定向实现证据

| 切片 | 已记录 command/result |
| --- | --- |
| Contracts | `cargo test --locked --package cosh-gateway-contracts`：6 个 integration test 通过；unit/doc-test target 通过。Package fmt、all-target Clippy、rustdoc 与 dependency-tree check 也通过。 |
| Gateway library integration | `cargo +1.88.0 test --locked --package cosh-gateway --no-fail-fast`：84 passed、0 failed。这是单 package suite，不是 workspace/full-system gate。 |
| Task reducer | Aggregate suite 包含 15 个 focused transition test，包括 unresolved/uncertain execution guard。 |
| SQLite storage | Storage suite 包含 15 个 focused test，包括 normal load/commit 的 snapshot replay verification。 |
| Runtime 与 ACP | Package suite 覆盖 private JSONL、ACP v1 codec/Bridge、固定 profile、有界 I/O/supervision 与可独立 cancel 的 Session Driver。Gateway all-target Clippy 与 package rustdoc 通过。 |
| Capability | `cargo +1.88.0 test --locked --package cosh-gateway capability --no-fail-fast`：12 passed、0 failed。只验证 in-memory decision/permit slice。 |

### 规划文档证据

| 检查 | 结果 |
| --- | --- |
| 模块文档包 | PASS：每个模块都有中英文 `design` 与 `acceptance` 文档 |
| 仓库文档 lint | PASS：`bash scripts/docs-lint.sh` |
| 仓库 link 检查 | PASS：`python3 scripts/docs-link-check.py` |
| 完整 owned-document link 检查 | PASS：8 份总体/开发者指南文档中的全部 relative link 可解析 |
| Markdown 卫生 | PASS：`git diff --check` 与 owned-file 行尾空白检查 |
| 实现声明复核 | PASS：区分基线与候选声明；ACP codec/Bridge/profile/Driver foundation 与缺失的已安装 entrypoint、production governance、real-adapter 证据明确区分 |

已记录的代码结果属于 scope-proportional package gate，不是 full workspace 或 live-system
validation。ECS validation、provider call 与手工 Terminal UX 未运行。

## 验收 Owner 与更新规则

Architecture Owner 维护本总报告。Module Owner 在产出实现证据的 PR 中更新详细报告。
只有全部模块报告满足 exit criteria，并且本报告记录精确聚合候选 commit 后，阶段才能通过。
