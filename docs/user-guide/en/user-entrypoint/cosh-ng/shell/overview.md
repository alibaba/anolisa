# cosh-shell Overview

cosh-shell is the AI-enhanced interactive terminal of cosh-ng. It layers AI analysis capabilities, tool approval controls, and inline card rendering on top of a native bash/zsh PTY, providing users with a secure and observable Agent interaction experience.

## Positioning

cosh-shell is the user-facing frontend layer:

- Manages the PTY host (bash/zsh subprocess)
- Connects to backends via AI adapters (default: cosh-core)
- Renders approval cards and AI analysis results
- Implements tool approval control protocol

## Run Modes

```bash
# Default startup (uses configured adapter and shell)
cosh-shell

# Explicitly specify adapter (positional argument)
cosh-shell raw cosh-core
cosh-shell raw claude
cosh-shell raw qwen

# Specify underlying shell
cosh-shell --shell zsh
cosh-shell raw co --shell bash

# Pass-through mode: execute single command then exit
cosh-shell -c 'ls -la'
cosh-shell -- git status

# Login shell mode
cosh-shell --login
cosh-shell -l

# Isolated mode (skip user rcfile)
cosh-shell --isolated
```

## Supported AI Adapters

| Adapter | Backend | Description |
|---------|---------|-------------|
| `cosh-core` | cosh-core process | Default adapter, full control protocol |
| `claude` | Claude Code CLI | Claude adapter |
| `qwen` | Qwen Code CLI | Qwen adapter |
| `fake` | Mock | For development testing, no backend required |

## Adapter Capabilities

| Capability | Description |
|-----------|-------------|
| `text_stream` | Text streaming output |
| `thinking_stream` | Thinking process streaming output |
| `session_resume` | Session resume |
| `tool_intent` | Tool call intent awareness |
| `user_question` | Ask user questions |
| `cancellable` | Supports cancelling running requests |
| `control_protocol` | Full control protocol support |

## Core Features

| Feature | Description | Documentation |
|---------|-------------|---------------|
| PTY Interaction | Native bash/zsh terminal | [interactive-mode.md](interactive-mode.md) |
| AI Analysis | Streaming command analysis | [ai-analysis.md](ai-analysis.md) |
| Tool Approval | Visual approval cards | [approval.md](approval.md) |

## Architecture Overview

```
┌────────────────────────────────────────────┐
│                 cosh-shell                 │
│  ┌───────────┐  ┌──────────┐  ┌─────────┐  │
│  │ PTY Host  │  │ Adapter  │  │   UI    │  │
│  │ (bash/zsh)│  │(cosh-core│  │(ratatui)│  │
│  └───────────┘  │/claude..)│  └─────────┘  │
│  ┌───────────┐  └──────────┘  ┌─────────┐  │
│  │  Hooks    │  ┌──────────┐  │Approval │  │
│  │  Engine   │  │  Tools   │  │ Broker  │  │
│  └───────────┘  └──────────┘  └─────────┘  │
└────────────────────────────────────────────┘
         │                │
         ▼                ▼
    bash/zsh PTY     cosh-core process
```

## Configuration

cosh-shell specific configuration is in the `[ui]` and `[shell]` sections of `~/.copilot-shell/config.toml`. See [Configuration](../configuration.md) for details.

## Project Trust

cosh-shell maintains project-level trust storage. On first launch in a new project directory, it prompts the user to confirm whether to trust the project. Trust status determines:

- Whether to load `.cosh/hooks` from the project directory
- Whether to apply project-level configuration overrides
