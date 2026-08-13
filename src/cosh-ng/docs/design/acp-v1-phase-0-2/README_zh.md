# ACP v1 Phase 0-2 规划集

[English](README.md)

## 状态

- 规划基线：`up/main` 的 `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`
- 候选工作树：基于该基线的未提交实现切片
- 文档日期：2026-08-13
- Phase 0-2 总体就绪度：**NOT ACCEPTED**
- 范围：架构、验收标准与第一轮实现证据

本规划集定义 cosh-ng 从交互式 Agent Shell 演进为本地优先 Agent OS Gateway
的前三个交付阶段。ACP v1 在这套架构中只是一个 Agent Runtime Adapter，
不承担渠道入口、持久 Task 存储、授权系统或远程控制传输。

固定的 `up/main` 基线上不具备这些能力。候选工作树只增加库级基础，不是生产
Gateway，而且尚无独立的候选 commit SHA。

## 候选实现快照

当前工作树包含以下局部基础：

- [`cosh-gateway-contracts`](../../../crates/cosh-gateway-contracts/src/lib.rs)：无副作用且有版本的
  Task/Runtime/Capability contract、有界 leaf string/digest，以及相互独立的内部和外部 identity；
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
  仅解析已安装的 `codex-acp` 与 `claude-agent-acp`，校验 canonical workspace/executable，
  使用 environment allowlist，且没有 shell、package runner 或 network bootstrap 路径。

Capability contract 与 package-exposed in-memory Broker/permit slice 已存在并通过 targeted
validation。工作树仍没有 Gateway daemon 或 network API、已安装的 local ACP entrypoint、支持独立
取消的 ACP Session Driver、production Permission Proxy、`CoshCoreBridge` public event mapping、完整
ACP-to-domain mapping、Shell Attachment、Web UI/API、钉钉/飞书 Adapter、restart/lease orchestration，也没有完成所有
production bypass closure。现有 `cosh-shell` 继续拥有 PTY 与兼容 cosh-core process path。

Contract 基础尚未对全部 collection 与 envelope 应用 aggregate admission limit，包括 vector、batch
和 Outbox payload。

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

当前 ACP slice 是带内置 launch profile 的 library-level interoperability probe，不是已安装的
production entrypoint。Filesystem/terminal callback、持久 `AgentSessionId` binding、Task event
mapping、restart/resume、独立取消与 real-adapter conformance 仍不在已实现切片内。更窄的
[Local ACP MVP](phase-1/acp-mvp/design_zh.md) 与完整 Phase 2 Bridge 分开定义。

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
| 0 | Protocol Contracts | [设计](phase-0/protocol-contracts/design_zh.md) | [报告](phase-0/protocol-contracts/acceptance_zh.md) | 有版本的领域与 Port 契约 |
| 0 | Identity and Correlation | [设计](phase-0/identity-correlation/design_zh.md) | [报告](phase-0/identity-correlation/acceptance_zh.md) | 无歧义的 actor 与生命周期身份 |
| 0 | Storage and Supervision | [设计](phase-0/storage-supervision/design_zh.md) | [报告](phase-0/storage-supervision/acceptance_zh.md) | 通过评审的持久化与进程 owner ADR |
| 1 | Gateway API | [设计](phase-1/gateway-api/design_zh.md) | [报告](phase-1/gateway-api/acceptance_zh.md) | 本地 admission 和 Task command 接口 |
| 1 | Task Execution Plane | [设计](phase-1/task-execution-plane/design_zh.md) | [报告](phase-1/task-execution-plane/acceptance_zh.md) | 持久 Task、event、lease 与 Outbox 状态 |
| 1 | Capability Broker | [设计](phase-1/capability-broker/design_zh.md) | [报告](phase-1/capability-broker/acceptance_zh.md) | 所有 OS 副作用的统一治理边界 |
| 1 | CoshCore Bridge | [设计](phase-1/cosh-core-bridge/design_zh.md) | [报告](phase-1/cosh-core-bridge/acceptance_zh.md) | 中立 Port 后的现有 JSONL Runtime |
| 1 | Local ACP Runtime MVP | [设计](phase-1/acp-mvp/design_zh.md) | [报告](phase-1/acp-mvp/acceptance_zh.md) | 单个已安装 local stdio text-prompt 路径 |
| 2 | ACP Client Bridge | [设计](phase-2/acp-client-bridge/design_zh.md) | [报告](phase-2/acp-client-bridge/acceptance_zh.md) | ACP v1 stdio Agent 互操作 |
| 2 | Shell Attachment | [设计](phase-2/shell-attachment/design_zh.md) | [报告](phase-2/shell-attachment/acceptance_zh.md) | 保留 PTY ownership 的 Shell attach/detach |
| 2 | Web and Presentation | [设计](phase-2/web-presentation/design_zh.md) | [报告](phase-2/web-presentation/acceptance_zh.md) | 可重放 Web/API view 与可靠投递 |

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
- 完整 provider、ECS 或手工 Terminal 验证属于需要另行明确请求的 gate；本规划集不表示这些验证已经执行。

## 外部资料

- [ACP 架构](https://agentclientprotocol.com/get-started/architecture)
- [ACP v1 初始化](https://agentclientprotocol.com/protocol/v1/initialization)
- [ACP v1 Transports](https://agentclientprotocol.com/protocol/v1/transports)
- [ACP 更新](https://agentclientprotocol.com/updates)
- [Warp Oz Platform](https://docs.warp.dev/platform/overview/)
- [Warp 架构与部署](https://docs.warp.dev/enterprise/enterprise-features/architecture-and-deployment)
