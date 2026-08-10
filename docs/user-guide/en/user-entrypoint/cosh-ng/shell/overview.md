# Interactive Terminal

[中文版](../../../../zh/user-entrypoint/cosh-ng/shell/overview.md)

`cosh` is a bash or zsh terminal with an Agent available for natural-language work. Use ordinary shell syntax for commands you know, and describe larger tasks when you want the Agent to investigate or act.

## A typical workflow

1. Change to the target directory and run `cosh`.
2. Run familiar commands normally.
3. Describe an investigation or task in natural language, including constraints such as “inspect only” or “ask before changing files.”
4. Review approval cards before allowing side effects.
5. Use `/session status` before leaving a long-running investigation.

Useful starts:

```bash
cosh
cosh --shell zsh
cosh --resume
```

## How input is routed

| Input | Result |
|---|---|
| `git status` | Runs in the foreground shell. |
| `why did the last command fail?` | Starts an Agent request with recent terminal evidence. |
| `/session list` | Runs a cosh control command. |
| Agent tool request | Runs automatically or shows an approval card according to the approval mode. |

Approved shell commands stay in the foreground shell, so prompts, output, job control, and `Ctrl+C` remain usable. See [Tool approval](approval.md) for the safety rules.

## Sessions and proactive help

- Sessions are persisted by cosh-core and scoped to the workspace where cosh started. Recovery restores model-visible conversation context, not terminal processes or old terminal output. See [Session recovery](session-recovery.md).
- `smart` is the default analysis mode. Use [AI analysis](ai-analysis.md) to choose how much proactive failure help appears.
- `/help` is the source of truth for commands in the installed version; use [Interactive commands](interactive-mode.md) for a concise reference.

## Next steps

- [Tool approval](approval.md)
- [AI analysis](ai-analysis.md)
- [Session recovery](session-recovery.md)
- [Session compaction](session-compaction.md)
- [Skills](../core/skills.md)
- [MCP](../mcp.md)
- [Extensions](../core/extensions.md)
