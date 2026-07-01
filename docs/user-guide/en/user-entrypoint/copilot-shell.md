# Copilot Shell (cosh)

Copilot Shell (cosh) is an AI-augmented interactive terminal assistant built on upstream Qwen Code v0.9.0. It provides a natural language interface for system operations while maintaining full shell compatibility.

---

## Overview

cosh combines a traditional shell experience with AI capabilities:

- **AI-Augmented Shell** — natural language commands alongside standard shell
- **Hook System** — extensible PreToolUse event hooks for integrating ANOLISA components (tokenless, agentsight)
- **Tool Approval** — interactive approval workflow before executing potentially dangerous commands
- **Context Awareness** — understands your working directory, git status, and environment

**Binaries:** `cosh`, `copilot`, `co` (all point to the same entry point)

---

## Installation

### Option 1: anolisa CLI (recommended)

```bash
anolisa install cosh
```

### Option 2: YUM (Alinux, requires ANOLISA YUM repo)

```bash
sudo yum install copilot-shell
```

### Option 3: Source build (developers)

```bash
cd src/copilot-shell && make build
```

---

## Quick Start

Start cosh:

```bash
cosh
```

Once inside cosh, you can:

```bash
# Use standard shell commands
ls -la
git status

# Use natural language
> "find all Python files modified in the last week"

# Mix both
> "compress all *.log files in /var/log older than 30 days"
```

---

## Features

### Tool Approval

When cosh identifies a potentially risky command, it prompts for confirmation:

```
⚠️  The following command will be executed:
    rm -rf ./build/
    
  [Y] Approve  [N] Reject  [E] Edit
```

### Hook System

cosh supports hooks that intercept PreToolUse events. ANOLISA components integrate via hooks — for example, the Tokenless extension intercepts tool calls to compress schemas before they reach the model.

### Skills

cosh supports a skill system for extending Agent capabilities:

| Path | Scope |
|------|-------|
| `.copilot-shell/skills/` | Project-level skills |
| `~/.copilot-shell/skills/` | User-level skills |
| `/usr/share/anolisa/skills/` | System-level skills |

### Key Bindings

| Key | Action |
|-----|--------|
| `Ctrl+L` | Clear screen |
| `Ctrl+C` | Cancel current input |
| `Tab` | Auto-complete |
| `↑/↓` | History navigation |

---

## Configuration

Configuration is managed through runtime settings and environment variables. Skills, hooks, and extensions live under `~/.copilot-shell/`.

```
~/.copilot-shell/
├── skills/           # User-level skills
├── extensions/       # Extensions (e.g., tokenless)
└── ...
```

---

## See Also

- [anolisa CLI](anolisa-cli.md)
- [Tokenless Integration](../token-saving/tokenless.md)
