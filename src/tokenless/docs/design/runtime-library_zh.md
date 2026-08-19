# Tokenless Runtime 库

[English](runtime-library.md)

## 目标

`anolisa-tokenless` 是框架无关的进程内 Tokenless SDK。平台 Wheel 同时包含 PyO3
Runtime 和固定版本的 RTK 可执行文件，因此 Python 应用不要求 `tokenless` 或 `rtk`
出现在 `PATH` 中。

公开的 `TokenlessSdk` 把宿主框架的四个生命周期映射为 Tokenless 行为：

| 生命周期 | 行为 |
|---|---|
| `before_model` | 可恢复的 Function Calling Schema 压缩，并按需发布恢复工具 |
| `before_tool_call` | 对 Adapter 明确指定的命令字段执行 RTK 改写 |
| `after_tool_call` | 响应压缩、TOON 候选选择和环境错误提示 |
| `retrieve` | 受可见 marker 授权的 byte-exact Stash 恢复 |

Tool Ready 在产品范围内硬关闭，不属于该 API。

## 协议与状态

Adapter 把框架对象转换为不可变的 `ModelRequest`、`ToolCall`、`ToolResult` 和
`RetrieveRequest`。`Attribution` 要求 Agent 和 Session 标识；工具生命周期还必须提供
Tool Use 标识。工具 Schema 统一为 OpenAI Function Calling JSON，但生命周期 Envelope
是 Tokenless 自身协议，不是 OpenAI 请求。

`tokenless-runtime` 统一持有 SQLite Stash 和统计记录器。Schema 与响应压缩共享 Stash，
候选被丢弃时会回滚对应 key。TOON 作为 Rust 库直接链接，不启动进程。只有 Adapter
提供 `command_field` 时才调用 RTK；每个改写后的 wrapper 都锚定到 Wheel 内置文件，并
携带本次执行的归属信息。

SDK 不保存进程级“当前 Session”。`before_model` 返回精确的可见 marker 集合，Adapter
把它保存在框架 Session 状态中，`retrieve` 只接受该集合中的 hash。宿主应用继续保存
原始工具结果供 UI 和业务逻辑使用，只把复制后的模型可见文本传给 `after_tool_call`。

非法输入、内置 RTK 缺失、挂载失败和工具重名会快速失败。压缩或单次命令改写失败属于
可选优化失败，会告警并保留原值。候选只有严格更短时才会采用；Schema 和响应截断还必须
能够恢复。

## 打包与验证

`make python-wheel` 会构建固定版本 RTK，把它暂存为
`anolisa_tokenless/_bin/rtk`，再生成 CPython 3.11 stable ABI 平台 Wheel。跨平台构建器
可以通过 `PYTHON_RTK_BINARY` 指定为同一 Wheel 目标构建的 RTK 文件。
`make test-python-runtime` 会在全新环境安装 Wheel，并在不依赖系统 RTK 的条件下验证
四个生命周期。

`anolisa-tokenless-agentscope` 支持 AgentScope 1.0.11 至 1.0.x 和 2.0.x。1.x Adapter
使用 Tokenless Toolkit、模型代理和公开的实例 Hook；2.x Adapter 使用 `on_model_call`
和 `on_acting`。2.0.0 在配对的 Middleware/Tool 中保存 marker，后续版本还会把它持久化
到 `AgentState.middle_context`。两者都开放完整 SDK；2.0.0 支持直接构造 Agent，App
集成从 2.0.1 开始。
