# 集成其他前端

[English](../../../../en/user-entrypoint/cosh-ng/core/overview.md)

`cosh-core` 是交互式 `cosh` 终端背后的 Agent 运行时。它负责连接 provider，运行
模型和工具循环，并管理 Hooks、Skills、MCP、扩展、registry、对话保存和压缩。
日常使用直接启动 `cosh`。只有集成其他前端或自动执行运行时控制操作时，才需要
直接调用 cosh-core。

## 支持的入口

```bash
# 执行一个 prompt 后退出
cosh-core --headless "检查磁盘使用情况，不要做任何修改"

# 持续运行的 JSONL 进程
cosh-core --headless

# 恢复或压缩持久化对话
cosh-core --headless --resume <session-id>
cosh-core --resume <session-id> --compact

# 从 stdin 处理一次不需要 provider 的 registry request
cosh-core --registry
```

未提供 `--headless` 时，非 TTY stdin 也会自动进入 headless 模式。交互式终端使用
持续运行的 headless 进程和 registry 协议，不使用 cosh-core 的直接 TTY 界面。

## 重要选项

| 选项 | 作用 |
|---|---|
| `--headless` | 强制使用 JSONL stdin/stdout 模式 |
| `--model <name>` | 覆盖已配置模型 |
| `--approval-mode <mode>` | 覆盖 `trust`、`auto`、`balanced` 或 `strict` |
| `--allowed-tools <names>` | 让精确匹配的已注册工具跳过审批 |
| `--tools <selection>` | 向模型暴露默认工具、空工具集或逗号分隔子集 |
| `--bare` | 禁用 project config、hooks、Skills、extensions 和 session persistence |
| `--resume <id>` | 选择当前工作空间中的已有对话 |
| `--compact` | 压缩已选择对话后退出 |
| `--registry` | 处理一次 registry request 后退出 |
| `--enable-shell-evidence-tool` | 为 cosh-shell 增加有边界的终端 evidence 访问 |
| `--verbose` | 提高 stderr 日志级别 |

`--allowed-tools` 修改审批策略，`--tools` 决定模型可以看到哪些工具。

## 运行流程

1. 确定工作空间并读取分层配置。
2. 加载不依赖 provider 的工具、Skills、扩展能力和 MCP 连接。
3. 选择 provider 并完成认证。
4. 读取 JSONL 消息，并持续输出模型和工具事件。
5. 需要时通过 control message 请求审批或用户输入。
6. 在安全边界保存完整对话和模型可见内容。
7. 没有活动任务时立即应用健康的 registry 变更。存在活动任务时，等它完成后再应用。

进程把日志写入 stderr 或日志文件。Headless 和 registry 模式的 stdout 只用于协议输出。

## 能力导航

| 能力 | 参考 |
|---|---|
| JSONL 和 registry messages | [Headless 模式](headless-mode.md) |
| Providers 和认证 | [Providers](providers.md) |
| 内置、MCP 和 extension tools | [Tools](tools.md) |
| MCP server 配置与管理 | [接入 MCP server](../mcp.md) |
| 可复用指令 | [Skills](skills.md) |
| 事件策略 | [Hooks](hooks.md) |
| 打包能力 | [Extensions](extensions.md) |
| Session 配置 | [配置](../configuration.md) |

协议集成者还应阅读开发者 [IPC 协议参考](../../../../../developer-guide/zh/cosh-ng/ipc-protocol.md)。
