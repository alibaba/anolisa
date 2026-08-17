# ACP Task Platform 总体验收报告

[English](acceptance-report.md)

## 报告身份

| 字段 | 值 |
| --- | --- |
| 基线 | `e90d9d9402c7fa1c8122267eb4e075c0adda51f5` |
| 候选 | 基于该基线的未提交共享工作树；尚无独立候选 SHA |
| 范围 | Phase 0、Phase 1 与 Phase 2 架构就绪度 |
| 被评估的代码变更 | Contracts、schema v9 Task/Run/Outbox/lease/input ledger、scheduler、Runtime port、ungoverned ACP path、异步 approval/input、package containment 与 local Gateway daemon/client |
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
| 中立 contract 与 identity | 无副作用的 `cosh-gateway-contracts`，冻结 Gateway/Task schema v1 与独立 Runtime schema v4，包含 versioned header、有界 leaf/aggregate、不同 ID newtype、Task/Runtime event 与 governance shape | 完整 compatibility manifest、ownership ADR 验收与 remote identity authority |
| Task reducer | `TaskAggregate` 与 local `TaskCoordinator` 串行处理 submit、read、event page、cancel、retry、准确 pending-input append、scheduler 和 settlement path；21 个 event × 9 个 state matrix 校验 rejection 不修改 aggregate | 完整 concurrent property/race 与通用 kill-point suite |
| SQLite Task store | Checksummed storage schema v9 使用 WAL/FULL、installation binding、atomic Task/Outbox/governance/input ledger、no-clobber backup/restore、只读脱敏 inspect、`SQLITE_FULL` rollback、typed execution result、release crate-private writer、单 payload 256 KiB 与单 commit 1 MiB bound，以及真实 `SIGKILL` 本地重启证据 | 完整 power-loss suite、disk-health/operator runbook、quarantine 与 filesystem race hardening |
| Runtime 与 private core transport | `RuntimeSupervisor`、private COSH JSONL 与 provider-neutral Runtime port 提供有界映射、identity fence、cancel、process settlement 与 scheduler 适配；package systemd containment fixture 已在 Ubuntu 24.04 arm64、systemd 255 上通过 | Shell ownership migration 与其他支持的 production environment 验证 |
| ACP v1 第一轮切片 | Rust 1.88 是已验证最低版本，SDK 2.0.0 已固定；ungoverned `doctor`/`run` profile descriptor-pin executable/workspace，Session Driver 覆盖 sequence/byte RAII 与 ACP failure matrix | Governed production admission、signed/offline distribution、已验收的真实 Codex/Claude conformance、人工 Terminal 验证与精确 candidate commit |
| Capability | 中立 Capability/Permit/Execution contract 与 ledger foundation 为后续可选 capability 保留；production task-only inventory 没有 side-effecting ExecutionTarget | Production Broker/ExecutionTarget wiring、checkpoint/ws-ckpt integration、通用 production gate、Shell/Skill/MCP/扩展工具与 reconciliation 证据 |

候选实现已包含可运行的 local Gateway daemon/client。Production `serve` 只接纳受约束的 brokered Core
task-only profile；其 immutable inventory 只有 `ask_user_question`，没有 production `ExecutionTarget`，
也不依赖 checkpoint/ws-ckpt。ACP 仅通过明确 ungoverned 的 `doctor` 与 `run` path 提供。Unix daemon
从 peer UID 解析稳定 local actor，在可续租且带 fence 的 Run lease 下调度 Outbox work，在 prompt 前持久化
`RuntimeBound`，并把重启后无法重连的 Runtime 收敛为 `runtime_lost`。它还提供 durable asynchronous
provider-native approval，Delivered 重放不会再次写 provider。Durable Task/Run/Outbox/lease/input、cancel、
retry 与 recovery 行为仍在范围内。通用 Capability/Permit/Execution contract 与 ledger row 作为后续基础
保留，不能作为 production execution loop 的证据。Checkpoint/ws-ckpt 支持属于后续可选 capability。
Shell Attachment、remote/channel API、通用 Broker 覆盖、已验收的真实 Codex/Claude evidence、人工 Terminal
验证与精确 candidate commit 仍缺。Private COSH JSONL 与 ACP 保持独立。
Runtime input request 与准确 pending identity 已持久化。Raw response 只存在 private typed dispatch
ledger，Task event 与 receipt 只保留 digest。

## 模块就绪度摘要

每个模块的详细报告是该模块的权威记录。

| 阶段 | 模块 | 候选就绪度 | 报告 |
| --- | --- | --- | --- |
| 0 | Protocol Contracts | `PARTIAL`；typed leaf contract 通过 targeted check，frozen schema/fixture 与完整 port 仍缺 | [报告](foundations/protocol-contracts/acceptance_zh.md) |
| 0 | Identity and Correlation | `PARTIAL`；已有独立 ID/binding，authenticated/durable mapping 与 fence 仍缺 | [报告](foundations/identity-correlation/acceptance_zh.md) |
| 0 | Storage and Supervision | `PARTIAL`；已有 storage schema v9、backup/restore、readonly inspect、`SQLITE_FULL`、recovery/fencing、本地真实 `SIGKILL` 与单环境 containment 实测；完整 power-loss、operator 与 ownership migration Gate 仍缺 | [报告](foundations/storage-supervision/acceptance_zh.md) |
| 1 | Gateway API | `PARTIAL`；已有 authenticated local Unix submit/get/events/cancel/retry/append-input/resolve-approval 与 scheduled brokered Core execution，remote identity 与更广 governed execution 仍缺 | [报告](task-plane/gateway-api/acceptance_zh.md) |
| 1 | Task Execution Plane | `PARTIAL`；已有 reducer、atomic store、Outbox worker、fenced lease、Runtime binding、cancel 与 fail-closed restart convergence；完整 platform execution 与 kill-point 证据仍缺 | [报告](task-plane/task-execution-plane/acceptance_zh.md) |
| 1 | Capability Broker | `PARTIAL`；通用 contract/ledger foundation 已存在，但 production `serve` 没有 ExecutionTarget 或 checkpoint/ws-ckpt path | [报告](task-plane/capability-broker/acceptance_zh.md) |
| 1 | CoshCore Bridge | `PARTIAL`；受约束的 Core profile 是 task-only 且只有 `ask_user_question`，side-effecting tool 与 interactive Shell ownership 不在其中 | [报告](task-plane/cosh-core-bridge/acceptance_zh.md) |
| 1 | Local ACP Runtime MVP | `PARTIAL`；已有 descriptor-pinned ungoverned profile、fake conformance 与 failure matrix；governed production admission、真实 Codex/Claude 和人工 Terminal Gate 仍未验收 | [报告](task-plane/acp-mvp/acceptance_zh.md) |
| 2 | ACP Client Bridge | `PARTIAL`；官方 v1 codec 与 supervised stdio 切片通过 focused test，domain/governance/recovery integration 仍缺 | [报告](adapters-and-presentation/acp-client-bridge/acceptance_zh.md) |
| 2 | Shell Attachment | `NOT IMPLEMENTED`；当前存在 direct Shell mode | [报告](adapters-and-presentation/shell-attachment/acceptance_zh.md) |
| 2 | Web and Presentation | `NOT IMPLEMENTED` | [报告](adapters-and-presentation/web-presentation/acceptance_zh.md) |

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

当前状态：**NOT ACCEPTED；已有可运行 local ACP slice，但非完整通用 Gateway**。

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

Local daemon 现已通过 neutral Runtime port 只调度受约束的 brokered Core task-only profile，在 fenced
lease 下消费 Outbox，在 prompt 前持久 Runtime binding，并在重启后无法重连 process 时 fail closed。
其 production inventory 只有 `ask_user_question`，没有 production `ExecutionTarget` 或 checkpoint/ws-ckpt
path。ACP `doctor` 与 `run` 保持 ungoverned interoperability path。通用 Capability/Permit/Execution
contract 与 ledger 是后续基础，因此不声称任何 checkpoint approval/permit/audit/execute/result loop 已通过。
Package unit containment fixture 已在 disposable Ubuntu 24.04 arm64、systemd 255 容器中通过。G1 仍未通过，
因为当前 scope 不治理 side-effecting tool、Shell、Skill、MCP、扩展工具或 legacy mutation path，真实
Codex/Claude、人工 Terminal、signed artifact、power-loss 与精确 candidate Gate 也仍开放。

### GM：Local ACP Runtime MVP

当前状态：**NOT ACCEPTED；installed 与 fake path 已存在，外部 Runtime 证明仍不完整**。

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

Release build 与 fake conformance 已执行。Dirty-worktree 上的 real-adapter 与交互观察不构成已验收
的真实 Codex/Claude 或人工 Terminal evidence，也不宣称 ECS Gate。Workspace package suite 在 canonical
serialized gate 下通过；默认并行 workspace run 仍暴露两个 timing-sensitive shell-host
assertion，workspace-wide Clippy 仍被无关的既有 warning 阻塞。

### 已记录的定向实现证据

| 切片 | 已记录 command/result |
| --- | --- |
| Contracts | `cargo test --locked --package cosh-gateway-contracts`；package fmt、all-target Clippy、rustdoc 与 dependency-tree check。 |
| Gateway library integration | `cargo test --locked --package cosh-gateway --lib`；all-target Clippy 与 package rustdoc。 |
| Task reducer | 21 个 event × 9 个 state 的 exhaustive reducer matrix 覆盖全部 current 组合，并验证非法 transition 不修改 aggregate。 |
| SQLite storage | Storage suite 覆盖 schema v9、WAL/FULL、`SQLITE_FULL` rollback、checksummed migration、readonly inspect、source-bound no-clobber backup/restore、writer bound 与本地真实 `SIGKILL` reopen/replay。 |
| Runtime 与 ACP | Package suite 覆盖 private JSONL、ACP v1 codec/Bridge、descriptor-pinned profile、workspace inode digest、sequence/byte RAII、ACP failure matrix、有界 supervision 与独立 cancel。 |
| Capability | 通用 Broker/Permit/Execution contract 与 ledger 检查只属于后续基础证据；不声称 production checkpoint adapter 或 ws-ckpt execution loop。 |
| Systemd containment | `scripts/test-gateway-containment.sh` 在 disposable Ubuntu 24.04 arm64、systemd 255 容器中报告 PASS。它验证 rendered package unit、同 UID user-manager positive control、transient-unit escape 失败，以及 Gateway `SIGKILL` 后 direct child/grandchild/double-fork/`setsid` cleanup；旧 cgroup 清空后 replacement 才 ready。 |
| 外部 Runtime | Release 与 fake harness path 已执行。Dirty-worktree 上的 Codex/Claude 与交互观察只作为探索性记录；真实 Codex/Claude conformance 与人工 Terminal 验证仍未验收。 |

### 规划文档证据

| 检查 | 结果 |
| --- | --- |
| 模块文档包 | PASS：每个模块都有中英文 `design` 与 `acceptance` 文档 |
| 仓库文档 lint | PASS：`bash scripts/docs-lint.sh` |
| 仓库 link 检查 | PASS：`python3 scripts/docs-link-check.py` |
| 完整 owned-document link 检查 | PASS：8 份总体/开发者指南文档中的全部 relative link 可解析 |
| Markdown 卫生 | PASS：`git diff --check` 与 owned-file 行尾空白检查 |
| 实现声明复核 | PASS：task-only `ask_user_question` inventory、缺少 production ExecutionTarget、后续 checkpoint/ws-ckpt capability 与单一 containment 环境，已和通用工具治理、已验收真实 Adapter、人工 Terminal、remote channel 及精确 candidate commit 明确区分 |

已记录命令包括 scope-proportional package gate 与上述环境中的一次 destructive containment run。
Exact-candidate 真实 Codex/Claude、人工 Terminal、signed artifact、power-loss 与 ECS validation
仍未验收。Worktree 仍未提交，因此这些
结果不能满足要求同一个精确 candidate commit 的 criterion。

## 验收 Owner 与更新规则

Architecture Owner 维护本总报告。Module Owner 在产出实现证据的 PR 中更新详细报告。
只有全部模块报告满足 exit criteria，并且本报告记录精确聚合候选 commit 后，阶段才能通过。
