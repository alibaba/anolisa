# Interactive Terminal

[中文版](../../../../zh/user-entrypoint/cosh-ng/shell/overview.md)

`cosh` is the primary cosh-ng experience: a normal bash or zsh session with an
Agent available in the same input line. It preserves terminal semantics while
adding natural-language routing, streaming Agent output, approval cards,
Skills, MCP tools, and session recovery.

## How input is routed

Input is routed by intent:

| Input | What cosh does |
|---|---|
| `git status` | Sends it to the foreground shell unchanged |
| `why did the last command fail?` | Starts an Agent turn with recent terminal evidence |
| `/session list` | Runs a cosh control command |
| Agent tool call | Shows a card or auto-runs it according to approval mode |

The shell stays authoritative for interactive commands. When the Agent proposes
a shell command, cosh hands an approved command back to the foreground PTY, so
its output, prompts, job control, and `Ctrl+C` remain visible and interactive.

## Start and resume

```bash
cosh                         # configured shell and cosh-core adapter
cosh --shell zsh             # choose zsh explicitly
cosh --isolated              # skip user rcfiles
cosh --resume                # choose a conversation for this workspace
cosh --resume <session-id>   # resume a known conversation
cosh -c 'uname -a'           # non-interactive passthrough
```

The underlying shell is selected in this order: `--shell`,
`COSH_SHELL_RAW_SHELL`, `shell.default`, then the detected login shell with
bash as fallback.

## Daily workflow

1. Enter the target directory and run `cosh`.
2. Use shell syntax for commands you already know.
3. Describe higher-level work in natural language; include constraints such as
   “inspect only” or “ask before changing files.”
4. Review approval cards before allowing side effects.
5. Turn repeatable instructions into a Skill.
6. Use `/session status` or `/status` before leaving a long investigation.

## Control surface

`/help` shows the exact commands supported by the installed version. The most
useful groups are:

| Goal | Commands |
|---|---|
| Runtime and health | `/status`, `/health`, `/stats [model\|tools]` |
| Authentication and modes | `/auth`, `/mode approval ...`, `/mode analysis ...`, `/config language ...` |
| Conversation lifecycle | `/session ...`, `/draft` |
| Reusable capabilities | `/skills ...`, `/extensions ...`, `/mcp ...` |
| Inspection | `/hooks`, `/recommendations ...` |

See [Interactive behavior](interactive-mode.md) for the complete public command
summary and [Tool approval](approval.md) for mode semantics.

## Conversation persistence

Agent conversations are persisted by cosh-core and scoped to the canonical
workspace. Resume them with `cosh --resume` or `/session resume <id>`. Recovery
reconstructs model-visible conversation context; it does not restore terminal
processes or transient UI state.

## Safety boundaries

- `recommend` asks before every Agent tool call.
- `auto` is the default and allows eligible low-risk tools while keeping shell
  commands and guarded operations behind approval.
- `trust` removes routine approval and therefore requires an explicit
  `/mode approval trust confirm` transition.
- Project hooks and workspace extension settings apply only to trusted project
  roots.
- Audit and diagnostic output is redacted before export.

## Next steps

- [Interactive behavior and slash commands](interactive-mode.md)
- [AI analysis modes](ai-analysis.md)
- [Tool approval](approval.md)
- [Session recovery](session-recovery.md)
- [Session compaction](session-compaction.md)
- [Skills](../core/skills.md)
- [Connect an MCP server](../mcp.md)
- [Extensions](../core/extensions.md)
- [Configuration](../configuration.md)
