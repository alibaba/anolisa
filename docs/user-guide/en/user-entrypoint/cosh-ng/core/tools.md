# Tool System

cosh-core includes a set of built-in tools for LLM to invoke during conversations. Tools are classified by security level, which determines the approval strategy.

## Built-in Tool List

| Tool Name | Classification | Description |
|-----------|---------------|-------------|
| `read_file` | ReadOnly | Read file contents (supports line ranges) |
| `grep` | ReadOnly | Regex search file contents |
| `edit` | FileEdit | Precise file editing via search-and-replace |
| `write_file` | FileEdit | Create or overwrite files |
| `shell` | ShellExec | Execute shell commands |
| `skill` | Other | Invoke registered skills |
| `todo` | Other | Manage task lists |
| `ask_user_question` | Other | Ask user a question and wait for response |
| `cosh_shell_evidence` | ShellEvidence | Get terminal output as evidence (requires `--enable-shell-evidence-tool`) |
| `mcp__<server>__<tool>` | Mcp | Tool discovered from a trusted stdio MCP server |

## Tool Classification

```rust
pub enum ToolKind {
    ReadOnly,       // Pure read, does not modify system state
    FileEdit,       // Modifies file contents
    ShellExec,      // Executes arbitrary shell commands
    ShellEvidence,  // Reads terminal output history
    Mcp,            // Calls an external MCP server tool
    Other,          // Side-effect-free auxiliary operations
}
```

Classification determines the default behavior under approval modes:

| Approval Mode | ReadOnly | FileEdit | ShellExec | ShellEvidence | MCP | Other |
|--------------|----------|----------|-----------|---------------|-----|-------|
| `trust` | Auto | Auto | Auto | Auto | Auto | Auto |
| `auto` | Auto | Auto | Approval | Auto | Approval | Auto |
| `balanced` | Auto | Approval | Approval | Auto | Approval | Approval |
| `suggest` | Auto | Approval | Approval | Auto | Approval | Approval |
| `strict` | Auto | Approval | Approval | Auto | Approval | Approval |

> **Note**: `ask_user_question` and `cosh_shell_evidence` tools always bypass the approval flow and auto-execute regardless of mode.

## Tool Call Protocol

When LLM decides to call a tool, Core notifies via streaming events:

```json
{"type":"stream_event","event":{"subtype":"tool_use_begin","tool_name":"shell","tool_use_id":"tu-1"}}
{"type":"stream_event","event":{"subtype":"tool_use_delta","content":"{\"command\":\"df -h\"}"}}
{"type":"stream_event","event":{"subtype":"tool_use_end"}}
```

If approval is required, Core sends a `can_use_tool` request:

```json
{"type":"control_request","request_id":"apr-1","request":{"subtype":"can_use_tool","tool_name":"shell","tool_input":{"command":"df -h"}}}
```

Shell replies with the approval result:

```json
{"type":"control_response","response":{"subtype":"tool_approval","request_id":"apr-1","response":{"behavior":"allow"}}}
```

`behavior` options: `allow`, `deny`, `ask`.

## Tool Results

After tool execution completes, results are injected into the conversation context for LLM to continue reasoning. Each tool returns:

```rust
pub struct ToolResult {
    pub output: String,   // Tool output content
    pub is_error: bool,   // Whether this is an error result
}
```

## Auto-approved Tools

Specify tools that are always auto-approved via the `--allowed-tools` argument:

```bash
cosh-core --headless --allowed-tools shell,edit
```

With this, `shell` and `edit` tools auto-execute under any approval mode without user confirmation.

## Tool Registration

Tools are managed uniformly via `ToolRegistry`. The default tool set is created via `ToolRegistry::with_defaults()`. `--enable-shell-evidence-tool` additionally registers the `cosh_shell_evidence` tool.

Custom tools can be injected via the extension system. See [extensions.md](extensions.md).

## MCP Tools

MCP tools are dynamically discovered only when `cosh-core --headless` starts.
Each configured server receives `initialize`, then `tools/list`; a model call to
the registered name is forwarded as `tools/call`. See [Configuration](../configuration.md#mcp-stdio-servers)
for the trusted-server configuration and supported transport boundary.
