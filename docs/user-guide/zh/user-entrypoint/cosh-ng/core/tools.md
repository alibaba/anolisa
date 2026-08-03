# Agent 工具

[English](../../../../en/user-entrypoint/cosh-ng/core/tools.md)

Agent 通过工具读取和修改文件、运行命令、访问网络或调用 MCP 服务。每次调用能否
执行，由工具类型、当前审批模式、允许名单和 Hooks 共同决定。

## 默认工具

| 工具 | 类型 | 用途 |
|---|---|---|
| `read_file` | ReadOnly | 读取有边界的文件范围 |
| `read_many_files` | ReadOnly | 一次读取多个文件 |
| `grep` | ReadOnly | 搜索文件内容 |
| `glob` | ReadOnly | 匹配文件系统路径 |
| `list_directory` | ReadOnly | 列出一个目录 |
| `edit` | FileEdit | 精确替换文件内容 |
| `write_file` | FileEdit | 新建或替换文件 |
| `save_memory` | FileEdit | 保存项目或全局记忆 |
| `shell` | ShellExec | 执行 Shell 命令 |
| `web_fetch` | Network | 获取 HTTP 资源 |
| `skill` | Other | 列出或加载 Skill |
| `todo` | Other | 维护当前任务清单 |
| `ask_user_question` | Other | 暂停并获取结构化用户输入 |

`cosh_shell_evidence` 需要通过 `--enable-shell-evidence-tool` 显式启用。配置 MCP
服务后，会出现形如 `mcp__<server>__<tool>` 的工具名。Extensions 也可以加入带命名
空间的外部工具。

## 读取范围

`read_file`、`read_many_files`、`grep`、`glob` 和 `list_directory` 以 cosh-core
启动时确定的工作区为根。随后在 Shell 中执行 `cd` 不会改变这个范围。绝对路径和符号
链接只有在解析后仍位于该工作区内时才能使用。越过工作区、跨挂载点、特殊文件和根目录
被替换等情况都会被拒绝。

搜索和批量读取都有数量限制。遇到上限、不可读的子目录或循环时，工具会明确标记结果
被截断，避免把局部结果当成完整结果。

## 审批行为

| 工具类型 | `trust` | `auto` | `balanced` / `suggest` / `strict` |
|---|---|---|---|
| ReadOnly | 执行 | 执行 | 执行 |
| FileEdit | 执行 | 执行 | 询问 |
| ShellExec | 执行 | 询问 | 询问 |
| Network | 执行 | 询问 | 询问 |
| MCP / Extension 外部工具 | 执行 | 询问 | 询问 |
| Other | 执行 | 执行 | 询问 |

未知工具名会被拒绝。Hooks 仍可阻止原本允许的调用，或要求用户再次确认。
`ask_user_question` 用来向用户提问，终端证据读取遵循独立的前端协议和范围限制。

用户选择的模式会映射为相应的审批策略。`recommend` 使用严格审批，`auto` 使用自动
策略，`trust` 使用信任策略。

## 暴露与审批的区别

使用 `--tools` 限制提供给模型的工具。

```bash
cosh-core --headless --tools read_file,grep,ask_user_question
cosh-core --headless --tools empty
```

只有明确需要某个工具跳过审批时，才使用 `--allowed-tools`。

```bash
cosh-core --headless --allowed-tools mcp__search__query
```

把 `shell`、网络工具或外部工具加入允许名单会授予真实权限，请谨慎使用。

## 工具调用过程

Core 先流式输出工具调用事件。需要用户决定时，它会再发送控制请求。

```json
{"type":"control_request","request_id":"apr-1","request":{"subtype":"can_use_tool","tool_name":"shell","tool_input":{"command":"df -h"}}}
```

前端使用同一个 request ID 回答。cosh-shell 会把这组交互显示为卡片，获批的 Shell
命令可以交给前台 PTY 执行。

工具输出会回到当前一轮对话。进入下一次模型请求前，系统会限制大小、处理敏感内容并
控制循环次数。MCP 输出进入 Agent 上下文前限制为 64 KiB。

## MCP 和 Extension 工具

受信任的 MCP 服务只能由系统或用户配置，项目配置不能添加。Stdio 和 Streamable HTTP
服务都会通过 MCP roots 能力收到启动工作区。外部服务提供的工具说明不会降低审批要求。

配置和生命周期管理见[接入 MCP server](../mcp.md)。Extension 提供的工具见
[Extensions](extensions.md)。
