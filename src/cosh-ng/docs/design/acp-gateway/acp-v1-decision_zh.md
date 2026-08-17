# ACP v1 接入决策

[English](acp-v1-decision.md)

关联架构：[COSH Gateway 与 ACP 架构](README_zh.md)

## 决策

cosh-ng 作为 **ACP Client** 接入外部 Agent，采用稳定 wire protocol v1 和本机 stdio
transport。Codex 与 Claude Code 通过版本受控的 Adapter 接入同一
`AgentRuntimePort`，其他 Agent 可以遵循相同 Contract 扩展。

协议版本与 SDK 包版本分别管理：

- wire compatibility 由 `initialize.protocolVersion` 决定，稳定基线为 `1`；
- 初始 Rust SDK 基线为 `agent-client-protocol = 2.0.0`；
- 不通过 crate 主版本推断 wire 版本；
- ACP v2 和 unstable feature 需要独立的兼容性与安全评估。

官方 ACP 仓库说明当前稳定协议版本为 v1，并要求以初始化协商的
`protocolVersion` 判断 wire compatibility。官方 Rust SDK 2.0.0 的 MSRV 为 Rust
1.88.0，默认稳定能力与实验性 v2 feature 分离。

## 为什么使用 ACP

- 把 COSH 与单一 Agent CLI 的私有输出格式解耦。
- 复用标准 Session、Prompt、流式更新、Cancel 和 Permission Request 语义。
- 让 Codex、Claude Code 和后续 Agent 共用同一 Task、Approval 和审计模型。
- 保留端侧 Agent 接入路径，使弱网或断网场景不依赖远端控制协议。
- 将 Provider 兼容性收敛到 Adapter 和 conformance suite，而不是分散到 Shell UI。

## ACP 在架构中的边界

```text
COSH Task Plane
    -> AgentRuntimePort
        -> ACP v1 Client Bridge
            <-> ACP Agent Adapter
                -> Codex / Claude Code / 其他 Agent
```

ACP 负责 Agent Client 与 Agent 之间的运行时互操作，不负责：

- Channel 消息、聊天线程或跨设备连接；
- Task 持久化、调度、幂等、租约和崩溃恢复；
- 用户身份、租户、RBAC 或 OS Capability；
- Checkpoint、Rollback、ECS 管理和投递回执；
- COSH 内部组件之间的通用 IPC。

因此 ACP 是 **Runtime 可替换性边界**，不是 COSH 整体控制协议。

ACP Runtime selection 与 Gateway capability profile selection 也相互独立。接纳 ACP
adapter 不表示任何 `ExecutionTarget` 已存在。`task-only-v1` deployment 不暴露 governed
side-effect tool；`ws-ckpt-v1` 只通过 Runtime 明确确认的 typed hosted-operation contract
暴露 checkpoint。

Provider-native ACP permission 不是 fallback execution target。没有 typed hosted-result
contract 的 adapter 不能把 `allow_once` 变成 COSH Permit，也不能替代 `ws-ckpt` 完成
checkpoint request。

## 稳定能力范围

| 能力 | COSH 处理方式 |
|------|---------------|
| `initialize` | 必须协商 v1，记录双方 capability 和实现版本 |
| `session/new` | 每个 Run 建立明确 SessionBinding |
| `session/prompt` | 提交有界文本任务并接收终态 |
| Session Update | 映射文本、计划、工具调用和状态更新为 COSH Runtime Event |
| Permission Request | 转入 Approval Service；缺少可信关联时拒绝 |
| Cancel | 从 Task 取消传播到 ACP，并保证子进程最终回收 |
| Error/Close | 映射为确定性 Runtime 终态，保留有界诊断 |

filesystem、terminal client capability、Session load/resume/fork、远程 HTTP transport、
MCP-over-ACP 和 v2 实验能力不属于稳定基线。引入这些能力时，需要补充数据边界、威胁
模型、兼容性和 conformance 证据。

## 映射约束

- 一个 Task 可以有多个 Run；一个 Run 在任一时刻至多绑定一个活动 ACP Session。
- ACP Session Update 必须先转换成 provider-neutral Runtime Event，不能直接写入渠道 UI。
- ACP Tool Call 不是执行授权。真正副作用仍需 Approval 和 target-bound Permit。
- Permission Request 必须关联 Task、Run、Actor、Target、Operation 和 canonical digest。
- Prompt 完成、进程退出和传输关闭是不同事实，由 Runtime reducer 决定唯一终态。
- 未知消息、越界 payload、无关联回调和不支持的 capability 必须明确拒绝或忽略，不能
  自动降级为允许。

## Adapter 策略

- Adapter 版本与上游 Agent CLI 版本进入显式兼容矩阵。
- 生产 Profile 使用规范化绝对路径、固定 basename 和受控环境变量。
- 启动时清理环境，再显式继承运行所需的 locale、代理和认证入口；禁止继承动态加载和
  Node 注入变量。
- Adapter 安装、升级和来源校验由独立流程负责，Gateway 运行时不临时下载包。
- 每次升级必须重新执行 fake conformance 和真实 Adapter conformance。

## Conformance 要求

一个 Adapter 只有满足以下条件，才能作为受支持的 COSH Runtime 发布：

1. 固定 Adapter 与 Agent CLI 版本，完成 v1 初始化、建会话和文本任务。
2. 文本、计划、工具、权限、取消和错误事件能稳定映射到 Runtime Event。
3. `allow_once` 与 `reject_once` 真实端到端通过，拒绝路径不会产生副作用。
4. timeout、cancel、crash、malformed frame 和 transport close 都产生确定性终态并回收进程。
5. Task、Run、Session、Agent、Adapter、Approval、Execution 和错误证据可以关联审计。
6. conformance 证据脱敏，并且可以在 CI 或受控环境重复执行。

## 兼容与回滚

ACP Runtime 通过 Profile 启用。发生协议或 Adapter 回归时，禁用对应 Profile 并保留
Task 和审计数据，其他 Runtime 实现继续工作。回滚不修改已持久化的历史事件，也不把
失败 Run 自动迁移到另一个 Agent。

Runtime profile 与 capability profile 独立 rollback。两者的 admitted pair 必须有显式
conformance entry；Gateway 不为已有 Run 临时构造新的组合。

## 扩展新的 Agent

新增 Agent 集成应优先提供 ACP Adapter，而不是在 Gateway 或 Shell 中增加 Provider 私有
分支。贡献需要包含版本 Profile、安装来源校验、capability matrix、fake conformance 和
至少一次真实 Agent conformance。Provider 特有事件在 Adapter 内归一化，核心 Runtime
只接收公共 command、event 和 error。

## 参考

- [ACP 版本规则](https://github.com/agentclientprotocol/agent-client-protocol#versioning)
- [ACP 官方 Rust SDK](https://github.com/agentclientprotocol/rust-sdk)
- [Rust SDK 2.0.0 manifest](https://docs.rs/crate/agent-client-protocol/2.0.0/source/Cargo.toml)
