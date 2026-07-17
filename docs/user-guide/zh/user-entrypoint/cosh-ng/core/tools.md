# 工具系统

cosh-core 内置一组工具供 LLM 在对话过程中调用。工具按安全等级分类，决定审批策略。

## 内置工具列表

| 工具名 | 分类 | 说明 |
|--------|------|------|
| `read_file` | ReadOnly | 读取文件内容（支持行范围） |
| `grep` | ReadOnly | 正则搜索文件内容 |
| `edit` | FileEdit | 基于搜索替换的精确文件编辑 |
| `write_file` | FileEdit | 创建或覆盖文件 |
| `shell` | ShellExec | 执行 shell 命令 |
| `skill` | Other | 调用已注册的技能 |
| `todo` | Other | 管理任务清单 |
| `ask_user_question` | Other | 向用户提问并等待回答 |
| `cosh_shell_evidence` | ShellEvidence | 获取终端输出作为证据（需 `--enable-shell-evidence-tool`） |
| `mcp__<server>__<tool>` | Mcp | 从已配置 MCP Server 发现的工具 |

## 工具分类

```rust
pub enum ToolKind {
    ReadOnly,       // 纯读取，不修改系统状态
    FileEdit,       // 修改文件内容
    ShellExec,      // 执行任意 shell 命令
    ShellEvidence,  // 读取终端输出历史
    Mcp,            // 调用外部 MCP Server 工具
    Other,          // 无副作用的辅助操作
}
```

分类决定了审批模式下的默认行为：

| 审批模式 | ReadOnly | FileEdit | ShellExec | ShellEvidence | MCP | Other |
|----------|----------|----------|-----------|---------------|-----|-------|
| `trust` | 自动 | 自动 | 自动 | 自动 | 自动 | 自动 |
| `auto` | 自动 | 自动 | 审批 | 自动 | 审批 | 自动 |
| `balanced` | 自动 | 审批 | 审批 | 自动 | 审批 | 审批 |
| `suggest` | 自动 | 审批 | 审批 | 自动 | 审批 | 审批 |
| `strict` | 自动 | 审批 | 审批 | 自动 | 审批 | 审批 |

> **注意**：`ask_user_question` 和 `cosh_shell_evidence` 工具始终绕过审批流程，无论何种模式均自动执行。

## 工具调用协议

LLM 决定调用工具时，Core 通过流式事件通知：

```json
{"type":"stream_event","event":{"subtype":"tool_use_begin","tool_name":"shell","tool_use_id":"tu-1"}}
{"type":"stream_event","event":{"subtype":"tool_use_delta","content":"{\"command\":\"df -h\"}"}}
{"type":"stream_event","event":{"subtype":"tool_use_end"}}
```

如果需要审批，Core 发送 `can_use_tool` 请求：

```json
{"type":"control_request","request_id":"apr-1","request":{"subtype":"can_use_tool","tool_name":"shell","tool_input":{"command":"df -h"}}}
```

Shell 回复审批结果：

```json
{"type":"control_response","response":{"subtype":"tool_approval","request_id":"apr-1","response":{"behavior":"allow"}}}
```

`behavior` 可选值：`allow`、`deny`、`ask`。

## 工具结果

工具执行完成后，结果注入对话上下文供 LLM 继续推理。每个工具返回：

```rust
pub struct ToolResult {
    pub output: String,   // 工具输出内容
    pub is_error: bool,   // 是否为错误结果
}
```

## 自动审批工具

通过 `--allowed-tools` 参数指定始终自动审批的工具列表：

```bash
cosh-core --headless --allowed-tools shell,edit
```

此时 `shell` 和 `edit` 工具在任何审批模式下都自动执行，无需用户确认。

## 工具注册

工具通过 `ToolRegistry` 统一管理。默认注册的工具集合通过 `ToolRegistry::with_defaults()` 创建。`--enable-shell-evidence-tool` 额外注册 `cosh_shell_evidence` 工具。

自定义工具可通过扩展系统注入，参见 [extensions.md](extensions.md)。

## MCP 工具

MCP 工具只会在 `cosh-core --headless` 启动时动态发现。每个已配置的 Server 先接收
`initialize`，再接收 `tools/list`；模型调用注册名称时会转发为 `tools/call`。受信任
Server 配置、Streamable HTTP 与 OAuth 设置详见[配置说明](../configuration.md#mcp-servers)。
