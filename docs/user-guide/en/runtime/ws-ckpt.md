# Workspace Checkpoints (ws-ckpt)

ws-ckpt provides millisecond-level workspace checkpoint and rollback for AI Agents. It leverages filesystem COW (Copy-on-Write) to create instant snapshots of the working directory, enabling safe experimentation and fast recovery.

---

## Overview

When AI Agents modify code, configurations, or data files, mistakes can be costly. ws-ckpt allows Agents (and users) to:

- Create instant snapshots before risky operations
- Roll back to any previous checkpoint in milliseconds
- Compare differences between checkpoints
- Auto-checkpoint on configurable triggers

---

## Prerequisites

- Linux (x86_64 or aarch64)
- btrfs filesystem on the workspace volume (for COW snapshots)
- Agent runtime: OpenClaw or Hermes (for plugin mode)

---

## Installation

### Option 1: anolisa CLI (recommended)

```bash
anolisa install ws-ckpt
```

### Option 2: YUM (Alinux, requires ANOLISA YUM repo)

```bash
sudo yum install ws-ckpt
```

### Option 3: Source build (developers)

```bash
cd src/ws-ckpt && make build
```

---

## Plugin Installation

Install the ws-ckpt plugin for your Agent runtime:

```bash
# For OpenClaw
ws-ckpt plugin install --runtime openclaw

# For Hermes
ws-ckpt plugin install --runtime hermes
```

---

## Skill Installation

To enable natural-language-driven checkpoint operations, install the ws-ckpt skill:

```bash
# Install from GitHub
# Skill URL: https://github.com/alibaba/anolisa/blob/main/src/ws-ckpt/src/skills/ws-ckpt/SKILL.md
```

Once installed, the Agent can interpret natural language commands like:
- "Save current workspace state"
- "Rollback to the last checkpoint"
- "Show me what changed since the last save"

---

## CLI Commands

| Command | Description |
|---------|-------------|
| `ws-ckpt checkpoint [--name <name>]` | Create a new checkpoint |
| `ws-ckpt rollback <checkpoint-id>` | Restore workspace to a checkpoint |
| `ws-ckpt list` | List all checkpoints |
| `ws-ckpt diff <id1> [<id2>]` | Show differences between checkpoints |
| `ws-ckpt delete <checkpoint-id>` | Delete a specific checkpoint |
| `ws-ckpt status` | Show current workspace status |
| `ws-ckpt config` | View/edit configuration |

### Examples

```bash
# Create a named checkpoint
ws-ckpt checkpoint --name "before-refactor"

# List existing checkpoints
ws-ckpt list

# Compare current state with a checkpoint
ws-ckpt diff ckpt-3a7f

# Rollback to a specific checkpoint
ws-ckpt rollback ckpt-3a7f

# Delete old checkpoints
ws-ckpt delete ckpt-1b2c
```

---

## Natural Language Usage (Agent-Driven)

When the ws-ckpt skill is installed, Agents can use checkpoints via natural language:

| Intent | Example Phrases |
|--------|-----------------|
| Create checkpoint | "Save the workspace", "Take a snapshot before I start" |
| Rollback | "Undo all changes", "Go back to the last good state" |
| List checkpoints | "Show all saved states", "List my checkpoints" |
| Diff | "What changed since the last save?" |

---

## Auto-Checkpoint

ws-ckpt supports automatic checkpoint creation:

```toml
# ~/.config/ws-ckpt/config.toml

[auto_checkpoint]
# Create checkpoint before each Agent tool invocation
on_tool_call = true

# Scheduled checkpoints (cron expression)
schedule = "*/10 * * * *"   # every 10 minutes

[cleanup]
# Auto-remove checkpoints older than N hours
max_age_hours = 24

# Maximum number of retained checkpoints
max_count = 50
```

---

## Important Notes

> **WARNING**: The workspace path configured for ws-ckpt must NOT be:
> - The Agent startup directory or any parent directory
> - System paths (`/`, `/usr`, `/etc`, `/var`)
>
> Setting the workspace to these paths may cause system instability or Agent malfunction.

---

## Configuration

Default config location: `~/.config/ws-ckpt/config.toml`

```toml
[workspace]
# Path to the managed workspace
path = "/home/user/projects/my-project"

[storage]
# Snapshot storage backend
backend = "btrfs"

[auto_checkpoint]
on_tool_call = true
schedule = ""

[cleanup]
max_age_hours = 24
max_count = 50
```

---

## FAQ

**Q: What happens if my filesystem is not btrfs?**
A: ws-ckpt falls back to rsync-based snapshots, which are slower but functionally equivalent.

**Q: Can I use ws-ckpt with multiple workspaces?**
A: Yes. Run `ws-ckpt config` to manage multiple workspace paths.

**Q: How much disk space do checkpoints use?**
A: With btrfs COW, only changed blocks are stored. Typical overhead is <5% of workspace size per checkpoint.
