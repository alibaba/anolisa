# Phase 2 ACP Client Bridge 验收报告

[English](acceptance.md)

相关设计：[ACP Client Bridge 设计](design_zh.md)。

## 1. 报告范围

- 审计的上游基线：`e3763b001c91f3c13dc6afbd57aac924162e9f59`
- 审计日期：2026-08-13
- 变更类型：第一轮 library implementation slice 与设计证据
- 实现验收：**NOT ACCEPTED**

本报告记录当前就绪度和退出 Phase 2 所需的证据，不代表 production ACP 支持或已安装 Runtime
entrypoint 已经存在。更窄的首个可用 local path 由
[Local ACP MVP 报告](../../phase-1/acp-mvp/acceptance_zh.md)单独跟踪。

## 2. 基线证据

基线包含 Shell owner 的 `AgentAdapter`、流式 `AgentEvent` 类型、cosh-core
Adapter、内部 JSONL protocol 和 provider session persistence。它不包含 ACP
dependency、ACP Client、ACP JSON-RPC router、ACP stdio process、capability
negotiation 或 conformance suite。

cosh-core 中的 `CONTROL_PROTOCOL_VERSION = 1` 是内部 shell-to-core 契约，
不能作为支持 ACP `protocolVersion: 1` 的证据。

候选工作树加入官方 `agent-client-protocol = 2.0.0`，把 cosh-ng Rust/RPM baseline
提升到 1.88，并实现 `AcpV1Codec`、`AcpV1RuntimeBridge` 与固定的 installed-adapter profile。
Bridge 内嵌唯一的 `RuntimeSupervisor` lifecycle implementation；focused fixture 覆盖 v1 negotiation、supervised stdio
exchange、session/prompt/update、permission correlation、cancellation settlement、identity
mismatch、unsupported callback 与 malformed/oversized frame。
候选工作树还定义 shared object-safe `AgentRuntimePort` 与 ACP adapter。后者映射有界
text/tool event，通过可信 port 完成 permission normalization，关联 one-shot decision，
隔离公共 identity，并在发布 terminal event 前结算 supervised child。

## 3. 当前就绪度

| 领域 | 基线状态 | 验收状态 | 通过所需证据 |
| --- | --- | --- | --- |
| 中立 `AgentRuntimePort` | Production 中不存在 | **PARTIAL** | Shared object-safe port、Core 与 ACP implementation 已存在；coordinator integration 与完整 fixture 仍缺 |
| ACP SDK/toolchain ADR 与 dependency | 不存在 | **第一轮 PASS** | SDK 2.0.0 与 Rust 1.88 已固定；release/license review 仍是 PR gate |
| 内置 Runtime profile | 不存在 | **PARTIAL** | Installed entrypoint、pinned Adapter bundle、canonical path 与 environment allowlist 有 focused test；signed/offline distribution policy 仍缺 |
| ACP v1 初始化 | 不存在 | **PARTIAL** | Exact v1 request、response 与错误版本拒绝通过 focused test；真实 Agent conformance 仍缺 |
| Capability snapshot | 不存在 | **PARTIAL** | Stable capability copy 与 additional-root gate 已有；完整 method matrix 仍缺 |
| stdio transport | 只有内部 JSONL | **PARTIAL** | Fake Agent exchange 使用唯一 hardened supervisor；crash/backpressure suite 仍缺 |
| ACP session binding | 只有 provider session state | **PARTIAL** | ACP session 只以 scoped digest 暴露在 COSH-owned binding ID 下；coordinator durability 仍缺 |
| Prompt 与 update 映射 | 只有 Shell-specific event | **PARTIAL** | Bounded text、tool observation 与 stop reason 映射为中立有序 Runtime event；完整 update golden 仍缺 |
| Permission callback 治理 | 只有 Shell approval bridge | **PARTIAL** | Trusted normalizer 生成 Capability request，Broker result 只选择有关联 one-shot choice；production Broker wiring 仍缺 |
| Filesystem callback | 没有 ACP callback 路径 | **NOT IMPLEMENTED** | Broker-only read/write test 和逃逸 PoC |
| Terminal callback | 没有 ACP callback 路径 | **NOT IMPLEMENTED** | Governed execution handle 生命周期测试 |
| Cancellation 结算 | 已有 provider-specific cancellation | **PARTIAL** | 待决 permission callback 会收到 ACP cancelled outcome；prompt/process race suite 仍缺 |
| Load/resume/replay | 只有 cosh-core provider resume | **NOT IMPLEMENTED** | Capability-gated ACP load/resume test |
| Runtime supervision | Shell owner 的 process lifecycle | **PARTIAL** | ACP 复用 `RuntimeSupervisor`；restart、lease-loss 与 recovery 仍缺 |
| Conformance suite | 不存在 | **PARTIAL** | 官方 SDK 类型与 focused fixture 已通过；上游 corpus/真实 Agent 仍缺 |

候选实现证明了基本 ACP v1 transport shape，但仍不满足端到端治理、持久化、恢复或
Attachment exit criteria。

## 4. Exit Criteria

| ID | 标准 | 必需证明 |
| --- | --- | --- |
| ACP-01 | 每个 connection 首先发送 wire version `1` 的 ACP `initialize` | 准确 request/response fixture 和错误版本拒绝 |
| ACP-02 | SDK package 版本和 wire version 保持独立 | Dependency policy test 或 review 加文档断言 |
| ACP-03 | 首版使用本地 stdio，不依赖草案状态的 Streamable HTTP | 配置和 transport integration test |
| ACP-04 | ACP `sessionId` 只能映射到 `AgentSessionId` | Type-level API review 和 ID 混淆负向测试 |
| ACP-05 | `TaskId`、`RunId` 与 event sequence 在 Agent process 重启后保持 | 持久恢复 integration test |
| ACP-06 | Optional ACP method 只在对端声明后调用 | Capability matrix test |
| ACP-07 | Prompt chunk、plan、tool call、usage 和 stop reason 确定性映射 | Golden mapping fixture |
| ACP-08 | `session/request_permission` 始终进入 Approval 与 Broker policy | 端到端 fake Agent test 和 direct-call prohibition review |
| ACP-09 | `fs/*` 永不在 Bridge 内直接执行 filesystem I/O | Broker fake 断言以及 traversal、symlink PoC |
| ACP-10 | `terminal/*` 使用 target-bound governed execution handle | Create/output/wait/kill/release 生命周期测试 |
| ACP-11 | Cancel 结算未完成 prompt、permission 与 callback 工作 | 没有 late execution 的 race 和 timeout test |
| ACP-12 | Malformed 或污染 stdout 时 fail closed；stderr 有界且 redaction | Adversarial subprocess fixture |
| ACP-13 | Backpressure 不会导致无界内存增长 | 带明确定义终止结果的 saturation test |
| ACP-14 | Load replay 和 resume-without-replay 可区分 | Event flag 和 Presentation replay test |
| ACP-15 | 不支持恢复时绝不静默重发 prompt | 到达显式 blocked 状态的 crash/restart test |
| ACP-16 | 禁用 ACP Runtime profile 可恢复现有 Runtime 路径 | Rollback smoke test |

所有标准都是退出 Phase 2 的强制条件。Optional ACP feature 可以保持关闭，但
任何已声明 feature 都必须通过完整 callback 和治理标准。

## 5. 必需测试证据

实现验收报告必须记录：

- Candidate 完整 commit SHA；
- `Cargo.lock` 中准确的 ACP SDK crate 版本；
- 准确的 targeted test command 和 test count；
- 官方 ACP v1 schema 或 conformance fixture revision；
- 已支持 capability matrix；
- Line size、stderr、queue depth 和 timeout 的 subprocess limit；
- Path escape、ID confusion、permission spoofing、output contamination、重复
  execution 和 cancellation race 的 adversarial proof；
- 尚未测试的 optional ACP feature 和 transport。

当前 focused command：

```text
cargo +1.88.0 test --package cosh-gateway runtime::acp
```

未提交候选工作树中，现有 ACP codec/driver suite 与 5 个 ACP-port test 均通过。
后者覆盖 mapping、identity substitution、one-shot correlation、缺少 once-only choice、
cancellation 与 settlement ordering。这仍是第一轮切片证据，不是完整 Phase 2
conformance suite。
独立的 Core-port lifecycle suite 不能作为 ACP conformance evidence。

## 6. 手工与在线验证

本实现切片没有请求或执行 provider、ECS、手工 Terminal 或 screenshot 验证。
未来 live gate 必须对准确 candidate commit 运行并记录脱敏证据后才能标记通过。

## 7. 剩余 Blocker

- 必须先验收 Phase 0 Runtime Port、ID、event、persistence 与 supervision
  contract。
- 必须具备 Phase 1 Task Plane、Capability Broker、Approval Service 与
  Execution Target。
- 固定 executable name、local resolution、pinned source installer 与 installed entrypoint
  已实现；signed/offline distribution policy 仍缺。
- Output、terminal lifetime 与 optional replay policy limit 需要批准具体值。

## 8. 验收决定

**PARTIAL IMPLEMENTATION / NOT ACCEPTED。** v1 codec 与 supervised stdio bridge
构成真实候选证据；只有在同一 candidate revision 上为 ACP-01 至 ACP-16 提供全部
实现证据后，才能验收 Phase 2。
