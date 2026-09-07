# Interactive Commands

[中文版](../../../../zh/user-entrypoint/cosh-ng/shell/interactive-mode.md)

Use this page to start `cosh` and control a running session. Run `/help` to see the exact commands supported by the installed version.

## Start `cosh`

| Command | Use |
|---|---|
| `cosh` | Start Enhanced Assisted (`◇ `) with Agent and slash-command routing. |
| `COSH_SHELL_INTEGRATION=native cosh` | Start Native without Cosh hooks, observation, or insights. |
| `cosh --shell zsh` | Select zsh explicitly. |
| `cosh --isolated` | Skip user rcfiles. |
| `cosh --login` | Start a login shell. |
| `cosh --resume [id]` | Open the session picker or resume the given session. |
| `cosh -c '<command>'` | Run one command through the shell and exit. |
| `cosh -- <program> [args...]` | Run a program directly and exit. |

If no shell is selected, `cosh` uses its configured or detected bash/zsh and falls back to bash.
Integration is selected at startup. Enhanced is the default. Set
`shell.integration = "native"` in the user config for persistent hook-free
sessions, or use the environment variable for one launch.

## Input and editing

- Native integration sends every input byte to the foreground bash or zsh.
- Enhanced Assisted (`◇ `) routes Shell syntax to the foreground Shell and can
  turn a natural-language request into an Agent request.
- At an empty Enhanced prompt, `Shift+Tab` switches to Shell-only (`◌ `).
  Ordinary input, including a leading `/`, then goes to the Shell while
  post-command insights remain available. Press it again to restore Assisted.
- A leading `/` runs a Cosh control command only in Enhanced Assisted. In
  Native and Enhanced Shell-only it remains Shell input.
- `Shift+Enter` inserts a newline when supported. Multiline paste remains one submission.
- Up-arrow history includes shell input and slash commands. `Ctrl+C` cancels the active command or Agent request.

## Enhanced integration slash commands

| Command | Purpose |
|---|---|
| `/help` | Show the installed command set. |
| `/agent` | Compose a one-shot cosh-core request with optional Skill and workspace references. |
| `/health` | Run local health checks. |
| `/status` (`/about`) | Show runtime, provider, and session status. |
| `/stats [model\|tools]` | Show model identity or tool activity. |
| `/auth` | Choose or update provider authentication. |
| `/config language [auto\|en-US\|zh-CN]` | Inspect or set the UI language. |
| `/mode approval [recommend\|auto\|trust]` | Inspect or change tool approval. |
| `/mode analysis [smart\|auto\|manual]` | Inspect or change proactive analysis. |
| `/session ...` | Create, list, resume, clear, or compact sessions. |
| `/recommendations [on\|off\|status\|privacy\|clear]` | Manage local prompt recommendations. |
| `/hooks <command>` | Inspect Hook findings and trust state. |
| `/extensions <command>` | Manage extension packages and settings. |
| `/skills [list\|detail\|enable\|disable]` | Manage Skills. |
| `/mcp [list\|connect\|inspect\|refresh\|disconnect\|login\|logout]` | Manage MCP servers. |

Commands such as `/details`, `/audit`, and `/send-to-shell` appear only when the current card or run provides their required context. `/mcp login` requires the shell-based OAuth flow described by the MCP guide.

## Compose a one-shot Agent request

Run `/agent` when the configured runtime is cosh-core. The Agent Composer opens
as a multiline editor without changing how later shell input is routed. Enter
sends the request, `Shift+Enter` adds a line, and `Esc` cancels it and restores
the shell prompt.

The first token may select one Skill, and any later whitespace-separated token
that starts with `@` requests a file or directory from the current workspace:

```text
/skill:repo-review inspect @Cargo.toml @src
```

- `/skill:<name>` must be the first token. The selected Skill is invoked before
  other Agent tools.
- `@path` must name an existing file or directory inside the workspace. Absolute
  paths, parent traversal, workspace-escaping symlinks, and unsupported paths
  are rejected.
- A submission accepts at most 16 valid references. Directory references are
  non-recursive unless the request explicitly asks for traversal.
- cosh-ng sends only validated path metadata. It does not read or embed file
  contents while building the request.
- Rejected references are shown with their path and reason before the Agent
  turn starts, and are omitted from its structured reference context.

Plain text without a Skill or references is also valid. Other provider runtimes
leave `/agent` unavailable; an enhanced session can still use an ordinary
natural-language request or multiline input instead.

For approval behavior, see [Tool approval](approval.md). For proactive failure help, see [AI analysis](ai-analysis.md).
