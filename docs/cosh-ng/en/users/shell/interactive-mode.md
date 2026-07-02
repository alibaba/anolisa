# Interactive Mode

cosh-shell runs bash/zsh on top of a native PTY and marks command boundaries using OSC escape sequences, enabling seamless integration of AI analysis and tool approval.

## PTY Host

cosh-shell creates a pseudo-terminal pair via `openpty()` and starts a bash or zsh subprocess on the slave end:

```
┌──────────────────────┐
│    cosh-shell        │
│  ┌────────────────┐  │       ┌──────────────┐
│  │  PTY master    │──────────│  bash/zsh    │
│  └────────────────┘  │       │  (PTY slave) │
│  ┌────────────────┐  │       └──────────────┘
│  │  OSC Parser    │  │
│  └────────────────┘  │
└──────────────────────┘
```

### Shell Selection

```bash
cosh-shell                    # Default: auto (auto-detect)
cosh-shell --shell zsh        # Use zsh
cosh-shell raw qwen --shell zsh
```

Shell selection priority:
1. `--shell` argument
2. `COSH_SHELL_RAW_SHELL` environment variable
3. Configuration file `shell.default`
4. Auto-detect (default bash)

### Run Modes

| Mode | Description |
|------|-------------|
| Default (no subcommand) | Start interactive shell with configured adapter |
| `raw [adapter]` | Explicitly specify adapter |
| `-c <command>` | Execute command then exit (pass-through to underlying shell) |
| `-- <command>` | Execute command directly then exit |
| `--isolated` | Isolated mode, do not load user rcfile |
| `--login` / `-l` | Start as login shell |

## OSC Marking System

cosh-shell injects a custom rcfile (bashrc/zshrc) into the child shell, marking command lifecycle via OSC 1337 escape sequences:

```
ESC]1337;COSH;<payload>BEL
```

Marked events include:
- Shell ready (prompt displayed)
- Command execution start (preexec)
- Command execution end (precmd, carries exit code)
- Working directory change

These markers enable cosh-shell to precisely identify:
1. When the user enters a command
2. When a command starts/ends
3. The command's exit code
4. Current working directory

## Input Classification

User input in the shell is classified into the following types:

| Type | Description | Example |
|------|-------------|---------|
| Shell Command | Normal shell command | `ls -la`, `git status` |
| Slash Command | Built-in control command starting with `/` | `/help`, `/mode` |
| Natural Language | Natural language question | Handled by AI adapter |
| Agent Marker | Agent execution marker | Internal use |

## Slash Commands

| Command | Description |
|---------|-------------|
| `/help` | Display help information |
| `/mode [approval\|analysis] [value]` | View or switch mode |
| `/config [key] [value]` | View or modify runtime configuration |
| `/hooks [list\|enable\|disable] [name]` | Manage hooks |
| `/extensions [list\|enable\|disable] [name]` | Manage extensions |
| `/skills [list\|enable\|disable] [name]` | Manage skills |
| `/debug [state\|events\|adapter]` | Debug information |
| `/auth` | Trigger authentication flow |

## Startup Flow

1. Parse command-line arguments (shell type, adapter, mode)
2. Install terminal restore handler (restore termios on SIGTERM/SIGHUP/panic)
3. Load configuration (`~/.copilot-shell/config.toml`)
4. Initialize logging (file output to `~/.copilot-shell/logs/`)
5. Create PTY session, start bash/zsh subprocess
6. Inject OSC marking script
7. Start AI adapter connection
8. Render startup banner (COSH logo + adapter/shell/approval mode info)
9. Enter main event loop

## Terminal Restore

cosh-shell automatically restores terminal state (termios) in the following scenarios:

- Process receives SIGTERM / SIGHUP / SIGQUIT
- Panic triggered
- Normal exit

Ensures terminal is not left in raw mode after abnormal exit.

## Native Mode vs Isolated Mode

| Feature | Native Mode (default) | Isolated Mode (`--isolated`) |
|---------|----------------------|------------------------------|
| User rcfile | Loaded (~/.bashrc etc.) | Skipped |
| PS1 prompt | Preserves user settings | Uses `cosh-osc$ ` |
| History | Loads $HISTFILE | Not loaded |
| Environment variables | Inherited | Minimized |

## Session Working Directory

cosh-shell creates a working directory for each session under `~/.copilot-shell/`, storing:

- OSC marking scripts
- Command output references (auto-cleaned after 24 hours)
- Terminal restore request files
- Shell handoff request files
