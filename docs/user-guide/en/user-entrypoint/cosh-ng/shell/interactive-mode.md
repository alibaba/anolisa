# Interactive Behavior and Commands

[中文版](../../../../zh/user-entrypoint/cosh-ng/shell/interactive-mode.md)

This page is a compact reference for starting cosh and controlling a running
session. Run `/help` for the exact command set supported by the installed
version.

## Launch modes

| Command | Behavior |
|---|---|
| `cosh` | Start the configured bash/zsh and Agent adapter |
| `cosh --shell zsh` | Select the underlying shell |
| `cosh --isolated` | Skip user rcfiles for an isolated session |
| `cosh --login` | Start a login shell |
| `cosh --resume [id]` | Pick or select a persisted Agent conversation |
| `cosh -c '<command>'` | Execute through the underlying shell and exit |
| `cosh -- <program> [args...]` | Execute a program directly and exit |

## Input and editing

- Shell syntax is sent to the foreground bash/zsh process.
- Natural-language input starts an Agent turn in `smart` or `auto` analysis
  mode.
- A leading slash invokes a cosh control command. Natural-language sentences
  that merely contain a slash are not misclassified as control commands.
- `Shift+Enter` inserts a newline when the terminal supports the negotiated key
  protocol; multiline paste is preserved as one logical submission.
- Up-arrow history includes both shell and slash-command input.

cosh marks command boundaries with private OSC messages injected into the child
shell. This lets it associate command text, exit status, working directory, and
captured output without parsing the prompt itself.

## Public slash commands

| Command | Purpose |
|---|---|
| `/help` | Show commands supported by the running version |
| `/draft` | Open the prompt draft workflow |
| `/health` | Run local health collectors |
| `/status` (`/about`) | Show runtime, provider, and session status; `/about` is an alias |
| `/stats [model\|tools]` | Show runtime model identity or current tool activity |
| `/auth` | Choose or update provider authentication |
| `/config language [auto\|en-US\|zh-CN]` | Inspect or set UI language |
| `/mode approval [recommend\|auto\|trust]` | Inspect or change approval behavior |
| `/mode analysis [smart\|auto\|manual]` | Inspect or change analysis routing |
| `/session ...` | Create, list (including `--all`), resume, clear, or compact conversations |
| `/recommendations [on\|off\|status\|privacy\|clear]` | Control local prompt recommendations |
| `/hooks <command>` | Inspect findings, state, feedback, and project Hook trust |
| `/extensions <command>` | Manage extension packages and settings |
| `/skills [list\|detail\|enable\|disable]` | Manage reusable Skills |
| `/mcp [list\|connect\|inspect\|refresh\|disconnect\|login\|logout]` | Manage MCP servers |

Some commands such as `/details`, `/audit`, and `/send-to-shell` appear only
when the current card or run provides the required context. Diagnostic and
compatibility commands are intentionally omitted from normal help.

## Skills

```text
/skills detail service-health
/skills disable service-health
/skills enable service-health
```

`/skills` requires the default cosh-core adapter. Skill state is resolved for
the canonical workspace and becomes visible to subsequent Agent turns.

## Terminal recovery

cosh restores terminal settings on normal exit, panic, `SIGTERM`, `SIGHUP`, or
`SIGQUIT`. If a terminal still looks corrupted after a hard kill, run `reset`
from the parent shell.

For conversation persistence and deletion guarantees, see
[Session recovery](session-recovery.md). For card behavior, see
[Tool approval](approval.md).
