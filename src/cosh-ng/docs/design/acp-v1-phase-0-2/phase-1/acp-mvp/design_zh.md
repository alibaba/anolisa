# ACP v1 本地 Runtime MVP 设计

[English](design.md) | [验收报告](acceptance_zh.md) |
[规划集](../../README_zh.md)

## 状态与交付决策

本模块定义一个范围明确的交付 Gate，用于启动一个本地已安装 ACP Adapter，并完成
一轮 text turn。它小于完整 Phase 2 ACP、Shell Attachment、Web、持久 Gateway
与 OS 治理 Gate。

候选工作树已经具备：

- 基于官方 ACP Rust SDK 2.0.0、面向稳定 ACP wire version 1 的 codec；
- 同一个 `AcpV1RuntimeBridge` 中组合一个 `RuntimeSupervisor`，由后者拥有
  child process、stdio、process group 与 reap result；
- 有界 initialization、单 session、单 active text prompt、流式
  `session/update`、permission correlation、cancellation frame 与 fail-closed
  decode；
- 面向本地已安装 `codex-acp` 与 `claude-agent-acp` Adapter 的内置 `Codex`
  和 `ClaudeCode` profile resolver。

这些仍是 library-level foundation。当前没有已安装的 COSH entrypoint 选择 profile
并驱动完整 turn。同步 Bridge 的 owner 阻塞等待 Agent stdout 时，尚无独立 cancellation
control path。Permission response 是有关联的 ACP wire response，尚不是 COSH approval
或 Capability Broker decision。也没有记录真实 Adapter 证据。

## MVP 结果

一个已安装的 COSH command 可以选择内置 profile，固定一个 canonical workspace，
通过 stdio 启动对应的本地 Adapter，发送一个 text prompt，流式输出有序 text update，
处理或拒绝一次性 permission request，独立 cancel，并报告唯一 terminal result。

本 MVP 被接受前，两个真实 Adapter 中至少一个必须通过完整验收矩阵。源码支持两个
profile 不代表两个 Adapter 都通过了 live conformance。

## 范围

MVP profile 固定如下：

| 维度 | MVP 契约 |
| --- | --- |
| Transport | 本地 subprocess stdio，使用 newline-delimited ACP v1 JSON-RPC |
| Adapter profile | 仅已安装的 `codex-acp` 与 `claude-agent-acp` |
| Workspace | Launch 前固定的单个 canonical absolute directory |
| Connection | 单一 supervised Adapter process 与 ACP connection |
| Session | 每个 Driver 一个 opaque ACP session |
| Concurrency | 一个 active prompt 与一条有序 event stream |
| Prompt content | 非空、有界 UTF-8 text only |
| Permission | Agent 提供的一次性 allow 或 reject decision |
| Cancellation | 独立 control command、有界 escalation 与 reap |
| Presentation | Local entrypoint 输出有界 text/event 与安全 diagnostics |

## 明确非目标

MVP 不包括：

- Codex 或 Claude Code binary 原生实现 ACP；
- 通过 `npx`、shell、package runner 或任何 network bootstrap 下载或执行 Adapter；
- filesystem callback、terminal callback、rich prompt content、additional directory、
  session load、session resume 或 multi-session；
- `allow_always`、`reject_always`、持久 trust rule 或 policy mutation；
- Web、渠道、Shell Attachment、Gateway daemon、远端 transport 或跨设备 replay；
- 持久 Task recovery、Run lease、进程无感 restart 或完整 Capability Broker 治理。

不支持的 feature 保持不声明。Agent 请求未声明的 filesystem 或 terminal method 时，
返回有关联的 method-not-found response，绝不进入 host I/O。

## Runtime Profile 边界

内置 profile resolver 是 MVP 唯一的 Adapter 选择 authority：

| Profile ID | 必需 executable | Launch 规则 |
| --- | --- | --- |
| `Codex` | `codex-acp` | 解析 basename 完全一致的已安装 regular executable |
| `ClaudeCode` | `claude-agent-acp` | 解析 basename 完全一致的已安装 regular executable |

显式 executable 必须是 absolute path。隐式解析只搜索 `PATH` 中的 absolute entry。
Resolver canonicalize executable 与 workspace，使用固定的空 argument list，清空继承环境，
只复制 common 与 profile-specific allowlisted variable。Prompt、ACP payload 与 Adapter
output 均不能增加 process argument、替换 executable 或改变 workspace。

这些 executable 是单独安装的 Adapter。文档与 UI 不得声称原生 `codex` 或 `claude`
command 已实现 ACP。

## Local Entrypoint

MVP 要求一个已安装、由 COSH 拥有的 entrypoint，概念输入为：

```text
RunAcpPrompt {
  profile,
  workspace,
  prompt
}
```

最终 executable 与 flag 名称由实现决定，但 entrypoint 必须：

1. 只接受内置 profile ID；
2. Spawn 前完成 profile resolve；
3. 由外部负责 Adapter 安装，缺失时返回 typed missing-adapter error；
4. 对外提供 streamed update、permission request、cancellation 与唯一 terminal result，
   且不把 SDK object 暴露为 public COSH contract；
5. 未选择 ACP entrypoint 或 profile 时，不改变 `cosh-shell raw cosh-core`。

该 entrypoint 是本地 process orchestration，不是 Phase 1 authenticated Gateway API 或 daemon。

## Session Driver Ownership

MVP 在当前 Bridge 之上增加一个 Driver：

```text
local entrypoint
  -> profile resolver
  -> ACP session driver
       -> AcpV1RuntimeBridge
            owns AcpV1Codec + RuntimeSupervisor
                 owns child + stdio + process group + reap
```

这是当前 composition 对应的 ownership model。Bridge 拥有其内嵌 Supervisor，不从独立
daemon supervisor 借用 channel。系统中只有一个 process owner 和一个 codec owner。

Session Driver 拥有 command serialization、event sequencing、deadline 与独立 cancellation
handle。它执行：

```text
resolve profile
  -> launch adapter
  -> initialize(protocolVersion = 1)
  -> session/new(canonical workspace)
  -> session/prompt(text)
  -> zero or more ordered updates/permission requests
  -> prompt terminal, cancellation settlement, or transport failure
  -> shutdown and reap
```

只有 Driver task/thread 可以修改 Bridge。独立 control handle 把 cancel 发送到 Driver command
queue，因此 Driver 等待 Agent stdout 时仍可以 cancel。必须直接获取同一个被阻塞 `&mut`
Bridge 的设计不满足 MVP。

## Streaming 与 Terminal 语义

- 每个接受的 `session/update` 在 delivery 前获得单调递增 local sequence。
- MVP 只展示 text agent-message chunk。其他有效 update 变为有界 diagnostic event 或显式
  unsupported event，不能静默转换为 text success。
- Queue depth 与 byte limit 必须明确。Saturation 应 cancel 并令 turn 失败，不能无界 buffer。
- 每个 prompt 只交付一个 terminal result，包括 completed、cancelled、Agent error、protocol
  failure、process exit 或 timeout。
- Terminal settlement 后收到的 update 必须拒绝，不能改变已报告结果。
- 保留证据中不含 raw prompt、environment value、无限制 stderr 或 Adapter payload。

## 独立 Cancellation

即使 Agent 没有输出，local entrypoint 也能接受 cancellation。Driver：

1. 记录 local cancellation request；
2. 发送 ACP `session/cancel`，并为每个 pending permission callback 发送 cancelled outcome；
3. 在有界 protocol grace 内等待 prompt settle；
4. Connection 退出时关闭或停止 protocol input；
5. 通过内嵌 `RuntimeSupervisor` escalation 到 process-group termination 与 kill；
6. Reap child 与 reader state；
7. 发出唯一 cancelled 或显式 cleanup-failure terminal result。

Cancellation 与 prompt completion race 采用 first-terminal-wins。Cancel 已经获胜后，任何
permission response 或 Agent update 都不能授权工作。

## Permission Proxy

MVP permission 边界是本地 once-only permission proxy，不声称具备持久 Task approval 或
完整 Capability Broker 治理。

每个 `session/request_permission` 按以下规则处理：

1. 校验 active session、prompt、JSON-RPC request ID、tool call 与唯一 option ID；
2. 只保留 Agent 提供的 `allow_once` 与 `reject_once` choice；
3. 把不可信 label 仅作为 display data；
4. 接受一个与 request 关联的 local user decision；
5. 返回 Agent 已提供的 selected option，或 cancelled/rejected outcome；
6. 拒绝 duplicate、unknown option、late decision 与 cross-session ID。

MVP 不向用户提供 `allow_always` 或 `reject_always`，也不能创建 durable rule。Agent 只提供
unsupported choice 时必须 fail closed。Evidence record 只包含有界 correlation 与 decision
class，不含 raw tool input 或 credential。

## Failure 边界

| Failure | 必需结果 |
| --- | --- |
| Adapter 缺失或 basename 错误 | Spawn 前以 typed profile error 失败 |
| Workspace 缺失或不是 directory | Spawn 前失败 |
| ACP version 错误或 initialization timeout | Terminate 并 reap；报告 compatibility failure |
| Malformed、oversized、invalid UTF-8 或 contaminated stdout | Fail closed 并终止 process group |
| Stderr flood | 只保留 bounded safe tail；绝不作为 ACP 解析 |
| Agent 在 prompt 中退出 | 唯一 transport/process terminal；不得推断 success |
| Permission 没有受支持的 once option | 不授权并 reject 或 cancel |
| Stdout 静默时收到 cancel | Driver 独立接收并在有界时间内 settle |
| Output queue saturation | Cancel/fail 并返回稳定 overload result |
| Unsupported callback | 有关联的 method-not-found；无 host side effect |

## 交付顺序

1. 冻结本 MVP contract 以及 entrypoint/event/error vocabulary。
2. 保留现有 resolver 与 Bridge composition，并记录准确 ownership。
3. 增加 Session Driver 与独立 control channel。
4. 增加已安装 local entrypoint 与安全 presentation。
5. 增加 once-only permission proxy 与 evidence record。
6. 完成确定性 fake-Agent failure/race coverage。
7. 针对至少一个已安装官方 Adapter，在精确 revision 上运行 conformance 并记录脱敏证据。

## 与后续 Gate 的关系

通过本 MVP 只证明本地 ACP prompt、stream、cancel 与 once-only permission 互操作，不通过
G1 或 G2。持久 Task mapping、Capability Broker authorization、filesystem/terminal callback、
restart、Shell/Web Attachment 与远端 presentation 继续使用原模块验收标准。
