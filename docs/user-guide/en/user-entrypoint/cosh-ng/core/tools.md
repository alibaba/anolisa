# Agent Tools

[中文版](../../../../zh/user-entrypoint/cosh-ng/core/tools.md)

cosh-core exposes a bounded tool registry to the model. Tool kind, approval
mode, explicit allow lists, hooks, and the cosh-shell frontend jointly decide
whether a call can run.

## Default tools

| Tool | Kind | Purpose |
|---|---|---|
| `read_file` | ReadOnly | Read a bounded file range |
| `read_many_files` | ReadOnly | Read several files in one call |
| `grep` | ReadOnly | Search file content |
| `glob` | ReadOnly | Match filesystem paths |
| `list_directory` | ReadOnly | List one directory |
| `edit` | FileEdit | Replace exact file content |
| `write_file` | FileEdit | Create or replace a file |
| `save_memory` | FileEdit | Store a project or global memory fact |
| `shell` | ShellExec | Execute a shell command |
| `web_fetch` | Network | Fetch an HTTP resource |
| `skill` | Other | List or load a Skill |
| `todo` | Other | Maintain an in-run task list |
| `ask_user_question` | Other | Pause for structured user input |

`cosh_shell_evidence` is opt-in through
`--enable-shell-evidence-tool`. Configured MCP servers add
`mcp__<server>__<tool>` names; extensions may add namespaced external tools.

## Workspace boundary for read tools

`read_file`, `read_many_files`, `grep`, `glob`, and `list_directory` are rooted
in the canonical workspace captured when cosh-core starts. A later shell `cd`
does not move that boundary. Absolute paths and symlinks work only when they
resolve inside the pinned workspace; escapes, mount crossings, special files,
and root replacement fail closed.

Search and batch results remain bounded. When a limit, unreadable subtree, or
cycle makes a result incomplete, the tool reports truncation instead of
presenting the partial result as exhaustive.

## Approval behavior

| Tool kind | `trust` | `auto` | `balanced` / `suggest` / `strict` |
|---|---|---|---|
| ReadOnly | Run | Run | Run |
| FileEdit | Run | Run | Ask |
| ShellExec | Run | Ask | Ask |
| Network | Run | Ask | Ask |
| MCP / extension external | Run | Ask | Ask |
| Other | Run | Run | Ask |

Unknown tool names are denied. Hooks can still block or escalate an otherwise
allowed call. `ask_user_question` is a control interaction, and terminal
evidence reads follow their own bounded frontend protocol.

The shell maps its user-facing modes to core policy: `recommend` behaves like
strict approval, `auto` maps to core auto, and `trust` maps to core trust.

## Exposure versus approval

Use `--tools` to limit the declarations sent to the model:

```bash
cosh-core --headless --tools read_file,grep,ask_user_question
cosh-core --headless --tools empty
```

Use `--allowed-tools` only when exact names should bypass approval:

```bash
cosh-core --headless --allowed-tools mcp__search__query
```

Allow-listing `shell`, a network tool, or an external tool grants real authority;
do not use it as a convenience workaround for approval prompts.

## Tool-call protocol

Core streams tool-use events, then sends a control request if policy requires a
decision:

```json
{"type":"control_request","request_id":"apr-1","request":{"subtype":"can_use_tool","tool_name":"shell","tool_input":{"command":"df -h"}}}
```

The frontend answers with the same request ID. cosh-shell renders this exchange
as a card and, for approved shell commands, can execute the command in its
foreground PTY instead of inside the core process.

Tool output is injected back into the current Agent turn and is subject to
size, redaction, and loop limits before another model request. MCP output is
bounded to 64 KiB before entering Agent context.

## MCP and extension tools

Trusted MCP server definitions come only from system or user configuration,
never project configuration. Both stdio and Streamable HTTP servers receive the
canonical workspace through the MCP roots capability. Tool descriptions from an
external server never downgrade its approval requirement.

See [Connect an MCP server](../mcp.md) for setup and lifecycle management.
Extension-provided tools are covered in [Extensions](extensions.md).
