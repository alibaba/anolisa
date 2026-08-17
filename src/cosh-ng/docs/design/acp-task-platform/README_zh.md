# ACP Task Platform 规划集

[English](README.md)

## 状态

- 规划基线：`e90d9d9402c7fa1c8122267eb4e075c0adda51f5`
- 候选工作树：基于该基线的未提交实现切片
- 文档日期：2026-08-16
- Phase 0-2 总体就绪度：**NOT ACCEPTED**
- 范围：架构、验收标准与第一轮实现证据

本规划集定义 cosh-ng 从交互式 Agent Shell 演进为本地优先 Agent OS Gateway
的前三个交付阶段。ACP v1 在这套架构中只是一个 Agent Runtime Adapter，
不承担渠道入口、持久 Task 存储、授权系统或远程控制传输。

固定基线上不具备这些能力。候选工作树增加了可运行的 local Gateway 和一个受约束的
task-only production profile，但还不是通用 production Gateway，而且尚无独立的候选
commit SHA。该 profile 只暴露 `ask_user_question`；本 PR 不接入 production
`ExecutionTarget`，也不依赖 checkpoint 或 ws-ckpt。

## 候选实现快照

当前工作树包含以下局部基础：

- [`cosh-gateway-contracts`](../../../crates/cosh-gateway-contracts/src/lib.rs)：无副作用且有版本的
  Gateway/Task schema v1、Runtime contract schema v4、Capability contract、有界 leaf
  string/digest，以及相互独立的内部和外部 identity；
- [`cosh-gateway` Task 与 storage](../../../crates/cosh-gateway/src/task.rs)：纯 Task reducer 与 local
  single-writer SQLite WAL store，在同一 transaction 中提交 event、projection、idempotency receipt
  和 Outbox intent；
- [`RuntimeSupervisor`](../../../crates/cosh-gateway/src/runtime.rs)：direct child launch validation、
  bounded stdout/stderr、process-group escalation/reap 与一次 process terminal observation；
- **private COSH JSONL control protocol v1** 的严格 codec，包含 exact initialization 与 typed
  runtime-local observation。它不是 ACP；
- 初始 [`AcpV1RuntimeBridge`](../../../crates/cosh-gateway/src/runtime/acp.rs)，使用官方 Rust SDK
  2.0.0 类型承载 ACP wire v1，继续由 `RuntimeSupervisor` 提供唯一 process lifecycle implementation，并覆盖 initialization、
  单 session、text prompt、update、permission correlation 与 cancellation。
- 内置 [`ACP Runtime profile resolver`](../../../crates/cosh-gateway/src/runtime/profile.rs)，
  仅解析已安装的 `codex-acp` 与 `claude-agent-acp`，使用 descriptor 固定 exact executable
  inode 与 workspace directory，workspace digest 同时绑定 canonical path、device 与 inode，
  使用 environment allowlist，且没有 shell、package runner 或 network bootstrap 路径。

Capability contract、durable schema-v9 Task/Run/Outbox/lease/input ledger slice、installed ACP
entrypoint、durable provider-native approval、neutral Core/ACP Runtime port 与 local Unix Gateway
daemon/client slice 已存在。本地控制切片支持 peer-authenticated Task
submit/get/events/cancel/retry/append-input/resolve-approval。Production `serve` 只接纳受约束的
brokered Core task-only profile，其 immutable inventory 只有 `ask_user_question`，没有 production
`ExecutionTarget`。ACP 明确保留为不受治理的 `doctor`/`run` interoperability path。Scheduler
使用 fenced Outbox lease，在 prompt 前持久 Runtime binding，并在重启后无法重连 process 时
fail closed。通用 Capability/Permit/Execution contract 与 ledger row 作为可复用的后续基础保留，
不能作为 production execution loop 的证据。Checkpoint 与 ws-ckpt 集成属于后续可选 capability，
本 PR 不实现也不要求它。这不代表已经覆盖 Shell、Skill、MCP、扩展工具或通用 Broker。工作树仍
没有 remote/network API、Shell Attachment、Web UI/API、钉钉/飞书 Adapter，也没有关闭全部
legacy execution path。现有 `cosh-shell` 继续拥有 PTY 与兼容 cosh-core process path。

Contract 与 Runtime reducer 已有 aggregate admission、sequence、byte 和 transition matrix；Task reducer
覆盖 21 种 event 与 9 种 state。Release build 中 raw Task writer 只在 crate 内可见；单条 Task/Outbox
payload 上限为 256 KiB，完整 commit 上限为 1 MiB。Runtime input request 持久化，但 raw response
只保存在 private dispatch row，Task event 与 receipt 只保留 digest。更广 collection/envelope
compatibility corpus 仍属于完整 Gate。

## 产品决策

COSH 应当拥有持久 Task 和 OS 治理边界，并允许 Shell、Web、钉钉、飞书和
自动化客户端通过稳定 Port 接入。Terminal UI、provider 进程或 ACP session
都不能成为产品状态的事实来源。

ACP 集成采用以下约束：

- ACP wire protocol v1，`initialize.protocolVersion = 1`；
- 在 `Cargo.lock` 中准确固定官方 Rust SDK 2.0.0，并把 cosh-ng workspace 与 RPM
  build baseline 提升到 Rust 1.88；
- 每一项可选 method 或 payload 都必须经过 capability negotiation；
- Phase 2 首先实现本地 stdio transport；
- Web、渠道和跨设备流量使用 COSH 自有 Gateway API。

ACP v2 和仍处于草案状态的 Streamable HTTP transport 不属于 Phase 0-2
交付契约。

已安装 ACP slice 包含内置 launch profile、持久 Runtime binding、Task event mapping、restart
convergence 与独立取消。其真实 Codex/Claude 和人工 Terminal 路径仍未验收。ACP
filesystem/terminal callback，以及面向 Shell、Skill、MCP 或扩展工具的通用治理仍不在已验收
切片内。更窄的 [Local ACP MVP](task-plane/acp-mvp/design_zh.md) 与完整 Phase 2 Bridge 分开定义。

## 阅读顺序

1. [跨阶段架构](architecture_zh.md)
2. [Warp 对比与产品定位](warp-comparison_zh.md)
3. Phase 0 各模块设计与就绪度报告
4. Phase 1 各模块设计与就绪度报告
5. Phase 2 各模块设计与就绪度报告
6. [总体验收报告](acceptance-report_zh.md)

## 模块清单

每个模块都有中英文设计文档和验收报告。报告区分固定的上游基线与候选工作树局部证据；
文档完整或存在一个 library slice 都不表示阶段通过。

| 阶段 | 模块 | 设计 | 验收 | 目标交付结果 |
| --- | --- | --- | --- | --- |
| 0 | Protocol Contracts | [设计](foundations/protocol-contracts/design_zh.md) | [报告](foundations/protocol-contracts/acceptance_zh.md) | 有版本的领域与 Port 契约 |
| 0 | Identity and Correlation | [设计](foundations/identity-correlation/design_zh.md) | [报告](foundations/identity-correlation/acceptance_zh.md) | 无歧义的 actor 与生命周期身份 |
| 0 | Storage and Supervision | [设计](foundations/storage-supervision/design_zh.md) | [报告](foundations/storage-supervision/acceptance_zh.md) | 通过评审的持久化与进程 owner ADR |
| 1 | Gateway API | [设计](task-plane/gateway-api/design_zh.md) | [报告](task-plane/gateway-api/acceptance_zh.md) | 本地 admission 和 Task command 接口 |
| 1 | Task Execution Plane | [设计](task-plane/task-execution-plane/design_zh.md) | [报告](task-plane/task-execution-plane/acceptance_zh.md) | 持久 Task、event、lease 与 Outbox 状态 |
| 1 | Capability Broker | [设计](task-plane/capability-broker/design_zh.md) | [报告](task-plane/capability-broker/acceptance_zh.md) | 所有 OS 副作用的统一治理边界 |
| 1 | CoshCore Bridge | [设计](task-plane/cosh-core-bridge/design_zh.md) | [报告](task-plane/cosh-core-bridge/acceptance_zh.md) | 中立 Port 后的现有 JSONL Runtime |
| 1 | Local ACP Runtime MVP | [设计](task-plane/acp-mvp/design_zh.md) | [报告](task-plane/acp-mvp/acceptance_zh.md) | 单个已安装 local stdio text-prompt 路径 |
| 2 | ACP Client Bridge | [设计](adapters-and-presentation/acp-client-bridge/design_zh.md) | [报告](adapters-and-presentation/acp-client-bridge/acceptance_zh.md) | ACP v1 stdio Agent 互操作 |
| 2 | Shell Attachment | [设计](adapters-and-presentation/shell-attachment/design_zh.md) | [报告](adapters-and-presentation/shell-attachment/acceptance_zh.md) | 保留 PTY ownership 的 Shell attach/detach |
| 2 | Web and Presentation | [设计](adapters-and-presentation/web-presentation/design_zh.md) | [报告](adapters-and-presentation/web-presentation/acceptance_zh.md) | 可重放 Web/API view 与可靠投递 |

## 阶段 Gate

| Gate | 退出阶段前必须满足 | 不得后移的问题 |
| --- | --- | --- |
| G0 契约冻结 | Schema、ID invariant、capability 词表、持久化 ADR、监督 ADR、fixture 和兼容策略完成评审 | Runtime 专用对象不得泄漏到 Gateway 或 Task 契约 |
| G1 本地持久 Gateway | Task 可在重启后恢复；command/event/outbox transaction 规则成立；每次 OS write 都需要 target-bound permit；可通过 Runtime Port 调用 cosh-core | API handler、presenter 或 Agent bridge 均不能直接写 Task 状态或执行 OS action |
| GM Local ACP Runtime MVP | 一个已安装 local entrypoint 在一个 canonical workspace/session/active text prompt 范围内运行 `codex-acp` 或 `claude-agent-acp`；独立 cancel、once-only permission decision、fail-closed transport 与 real-adapter conformance 通过 | 不假定 Codex/Claude 原生 ACP，不允许 package runner/network bootstrap、filesystem/terminal capability、load/resume、Web/daemon dependency 或持久 permission rule |
| G2 ACP 与 Attachment | ACP v1 stdio conformance 通过；permission 和 terminal request 进入 COSH 治理；Shell 与 Web 面向同一 Task 完成 attach、detach、replay、approval 和 cancel | 不用 ACP 传输远端渠道；ACP Session ID 绝不能充当 Task ID |

## 变更控制

- 后续阶段不得无兼容决策地重定义已经冻结的 ID 或 event，并且任何变化都要更新 fixture。
- 每个实现 PR 必须引用自己满足的模块验收项，并附精确命令与证据。
- 验收证据必须记录被测 commit。只完成设计评审不能把 Runtime 行为标记为通过。
- Exact candidate、真实 Codex/Claude conformance、人工 Terminal 验证、signed artifact 与 power-loss
  evidence 仍是未验收 external Gate；ECS 与 dirty worktree 上的探索性观察不能关闭这些 Gate。

## 外部资料

- [ACP 架构](https://agentclientprotocol.com/get-started/architecture)
- [ACP v1 初始化](https://agentclientprotocol.com/protocol/v1/initialization)
- [ACP v1 Transports](https://agentclientprotocol.com/protocol/v1/transports)
- [ACP 更新](https://agentclientprotocol.com/updates)
- [Warp Oz Platform](https://docs.warp.dev/platform/overview/)
- [Warp 架构与部署](https://docs.warp.dev/enterprise/enterprise-features/architecture-and-deployment)
