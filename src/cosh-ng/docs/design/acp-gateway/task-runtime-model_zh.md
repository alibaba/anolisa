# Task 与 Runtime 模型

[English](task-runtime-model.md)

关联架构：[COSH Gateway 与 ACP 架构](README_zh.md)

## 目的

本文定义 Gateway 与 Agent Runtime 之间的持久身份和 provider-neutral Port，避免进程、
Provider 或渠道状态形成第二套 Task 状态机。

## 身份层级

```text
Installation
  -> Actor
  -> Task
      -> Run
          -> RuntimeBinding
          -> Turn
          -> Agent SessionBinding
          -> Request / ToolUse / Approval / Execution
```

每种 ID 只有一个语义和一个命名空间：

| 身份 | 权限与生命周期 |
| --- | --- |
| `InstallationId` | 一个已安装 Gateway 的权限域 |
| `ActorId` | Installation 内经过认证的调用主体 |
| `TaskId` | 用户可见的持久工作单元 |
| `RunId` | Task 的一次执行尝试 |
| `RuntimeBindingId` | 一个 Run 与一个 Runtime 实例的 fenced binding |
| `TurnId` | Run 内一次 prompt 到 terminal 的交互 |
| `SessionBinding` | 与 Provider 或 ACP Session ID 的映射 |
| `RequestId` | 一次 callback 或 input request |
| `ApprovalId` | 针对一个有界请求的持久决策记录 |
| `PermitId` | 一次执行的单次消费权限 |
| `ExecutionId` | 一次外部副作用尝试 |

不同领域的 ID 不得互相替代。外部 ID 只保存为有界 reference，并始终绑定内部 identity。

## Task、Run 与 Turn

- Task 跨客户端断线、Runtime 退出和 daemon 重启持续存在。
- 一个 Task 可以有多个 Run，但只能有一个 active Run。
- 一个 Run 拥有一个 lease generation 和零或一个 active RuntimeBinding。
- Turn 属于一个 Run。Turn 完成不自动终结一个多 Turn Run。
- Retry 创建新 Run，不重新打开 terminal Run。

Task event 是 append-only 事实。纯 reducer 在更新 projection 前验证 expected revision、
状态转换和 identity correlation。

## AgentRuntimePort

Gateway 只通过有版本、provider-neutral 的 command 和 event 与 Runtime 通信。

Command 包括：

- initialize/start；
- prompt 或 Turn input；
- 对 pending input request 的准确响应；
- cancellation 和 shutdown；
- approval acknowledgement 与 typed brokered result delivery。

Event 包括：

- 有界 observation；
- tool-use 生命周期；
- provider-native permission request；
- brokered operation request；
- 准确 input request；
- 唯一的 Run 或 Turn terminal outcome。

Provider 私有 payload 在 Bridge 内归一化。渠道 presentation 再由持久 event 派生。

Runtime initialization 还需要声明有界 hosted-operation inventory。Gateway 在接收 Task work
前，将其与 selected capability profile 精确比较。`task-only-v1` 要求 inventory 为空；
`ws-ckpt-v1` 要求准确版本的 checkpoint request 与 typed-result capability。缺失、额外或
downgrade operation 必须拒绝 admission，不能由 presentation logic 隐藏。

Runtime 只能请求 operation，不能选择具体 `ExecutionTarget`、socket、service、credential
或 fallback path。这些内容属于绑定 admitted profile 的可信 daemon configuration。

## Runtime binding fence

Callback 只有在以下字段全部匹配持久状态时才被接受：

- Actor、Task 和 Run；
- RuntimeBinding ID 与 generation；
- 当前 Run lease generation；
- request identity 与 expected revision；
- 单调 event sequence。

Lease renewal 可以改变 lease revision，但不改变 authority generation。新 generation 会
fence 上一 Runtime 的全部 callback。

## Terminal 语义

Prompt completion、process exit、transport close、cancellation 与 execution uncertainty
是不同事实。Runtime adapter 最多发出一次 terminal observation，由 Task reducer 决定
持久 Task outcome。

取消或 terminal settlement 后的 late observation/callback 必须 fail closed。响应丢失后的
重放返回持久状态，不得再次写 Runtime。

## 版本与边界

- Gateway/Task schema 与 Runtime schema 独立演进。
- 每个 string、collection、frame 和 aggregate 都有显式上限。
- Unknown version 与 unsupported operation 返回稳定 typed error。
- Runtime breaking change 只升级 Runtime schema，不隐式改变 ACP wire 或 Task schema。
- Machine-readable fixture 覆盖编解码、unknown field、边界与前后向兼容。

## 验收不变量

- 两个 Provider Session 不能在内部 identity 上碰撞。
- Stale Runtime 不能修改被替换的 Run。
- Retry 创建新 Run 和新 fence。
- Late permission/input response 被拒绝且没有 partial mutation。
- Port 只能观察到一个 terminal outcome。
- Core 与 ACP Adapter 通过同一 neutral-port contract suite。
- Run 启动前，Runtime hosted-operation inventory 必须与 admitted capability profile 精确匹配。
