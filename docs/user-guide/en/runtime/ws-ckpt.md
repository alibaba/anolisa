# Workspace Checkpoints (ws-ckpt)

ws-ckpt provides millisecond-level workspace checkpoint and rollback for AI Agents. It leverages filesystem COW (Copy-on-Write) to create instant snapshots of the working directory, enabling safe experimentation and fast recovery.

---

## Overview

When AI Agents modify code, configurations, or data files, mistakes can be costly. ws-ckpt allows Agents (and users) to:

- Create instant snapshots before risky operations
- Roll back to any previous checkpoint in milliseconds
- Compare differences between checkpoints
- Auto-checkpoint via plugin integration

---

## Prerequisites

- Linux (x86_64 or aarch64)
- btrfs filesystem on the workspace volume (for native COW snapshots), or any filesystem (ws-ckpt will create a btrfs loop image automatically)
- Agent runtime: OpenClaw or Hermes (for plugin mode)

---

## Installation

### Option 1: anolisa CLI (recommended)

```bash
sudo anolisa --install-mode system install ws-ckpt
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

# Uninstall
ws-ckpt plugin uninstall --runtime openclaw
```

`plugin install` first runs a detect script to verify prerequisites (exit 2 = missing prerequisite, abort; exit 1 = not installed but installable, continue), then runs the install script. Scripts live under `/usr/share/anolisa/adapters/ws-ckpt/<runtime>/`.

---

## CLI Commands

| Command | Description |
|---------|-------------|
| `ws-ckpt init -w <workspace>` | Initialize a workspace for checkpointing |
| `ws-ckpt checkpoint -w <workspace> -s <snapshot-id> -m <message> [--metadata <json>]` | Create a new checkpoint |
| `ws-ckpt rollback -w <workspace> -s <snapshot> [--preview]` | Restore workspace to a checkpoint |
| `ws-ckpt rollback -w <workspace> -n <num-ancestors>` | Rollback N ancestors |
| `ws-ckpt list [-w <workspace>] [--format table\|json]` | List all checkpoints |
| `ws-ckpt diff -w <workspace> -f <from> [-t <to>]` | Show differences between checkpoints |
| `ws-ckpt delete [-w <workspace>] -s <snapshot> [--force]` | Delete a specific checkpoint |
| `ws-ckpt status [-w <workspace>] [--format table\|json]` | Show current workspace status |
| `ws-ckpt cleanup -w <workspace> [--keep 20]` | Remove old checkpoints |
| `ws-ckpt config [-g \| -w <workspace>] [--enable-auto-cleanup] [--auto-cleanup-keep <N\|Nd>]` | View/edit configuration |
| `ws-ckpt plugin install --runtime openclaw\|hermes` | Install runtime plugin |
| `ws-ckpt plugin uninstall --runtime openclaw\|hermes` | Uninstall runtime plugin |
| `ws-ckpt recover [-w <workspace> \| --all] [--force]` | Recover from interrupted operations |
| `ws-ckpt reload` | Reload daemon configuration |
| `ws-ckpt daemon [--mount-path ...] [--socket ...] [--log-level ...]` | Start the daemon process |

### Examples

```bash
# Initialize a workspace
ws-ckpt init -w /home/user/projects/my-project

# Create a checkpoint
ws-ckpt checkpoint -w /home/user/projects/my-project -s snap-001 -m "before refactor"

# List checkpoints
ws-ckpt list -w /home/user/projects/my-project

# Diff between two snapshots
ws-ckpt diff -w /home/user/projects/my-project -f snap-001 -t snap-002

# Rollback to a specific checkpoint
ws-ckpt rollback -w /home/user/projects/my-project -s snap-001

# Preview rollback without applying
ws-ckpt rollback -w /home/user/projects/my-project -s snap-001 --preview

# Cleanup old checkpoints, keep last 20
ws-ckpt cleanup -w /home/user/projects/my-project --keep 20

# Enable auto-cleanup for workspace
ws-ckpt config -w /home/user/projects/my-project --enable-auto-cleanup --auto-cleanup-keep 7d
```

### diff Output Markers

| Marker | Meaning | Color |
|--------|---------|-------|
| `+` | File/directory added | Green |
| `-` | File/directory deleted | Red |
| `M` | Content modified | Yellow |
| `R` | Renamed | Cyan |

> diff ships a smart resolver that maps btrfs low-level transient inode references (such as `o261-118-0`) to real file paths and dedupes multiple operations on the same file. Rollback previews (`rollback --preview`) use the same marker semantics.

---

## Configuration

### Daemon Configuration

The daemon configuration file is located at `/etc/ws-ckpt/config.toml`. This is a system-level configuration for the ws-ckpt daemon process.

There is no user-side global config file. Auto-checkpoint and cleanup behavior are controlled per-plugin:

### OpenClaw Plugin Configuration

```json
// ~/.openclaw/ws-ckpt.json
{
  "autoCheckpoint": true,
  "workspace": "/home/user/projects/my-project"
}
```

### Hermes Plugin Configuration

```bash
hermes config set plugins.ws-ckpt.workspace /home/user/projects/my-project
```

### CLI-Based Configuration

Configuration has two layers: **global** (`/etc/ws-ckpt/config.toml`, daemon-wide defaults) and **local** (per-workspace `policy.toml` overrides). Running `ws-ckpt config` without a scope prints a read-only overview; `-g` views/edits the global config; `-w` can only override `auto_cleanup` and `auto_cleanup_keep` — the remaining fields (interval / image / health check) are daemon-wide and can only be set via `-g`; `-w <workspace> --reset` removes the workspace override and falls back to the global config.

```bash
# Enable auto-cleanup, keep checkpoints for 7 days
ws-ckpt config -w /home/user/projects/my-project --enable-auto-cleanup --auto-cleanup-keep 7d

# Global config
ws-ckpt config -g --enable-auto-cleanup --auto-cleanup-keep 20
```

---

## Important Notes

> **WARNING**: The workspace path configured for ws-ckpt must NOT be:
> - The root path (`/`)
> - Inside the daemon's mount_path
> - The Agent startup directory or any parent directory (validated at plugin level)
>
> These constraints are enforced by the daemon. Attempts to use invalid paths will be rejected.

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

## FAQ

**Q: What happens if my filesystem is not btrfs?**
A: ws-ckpt creates a btrfs loop image on the host filesystem and loop-mounts it, providing full COW snapshot functionality regardless of the underlying filesystem type.

**Q: Can I use ws-ckpt with multiple workspaces?**
A: Yes. Use `-w` flag with each command to specify the workspace, or configure multiple workspaces via plugins.

**Q: How much disk space do checkpoints use?**
A: With btrfs COW, only changed blocks are stored. Typical overhead is <5% of workspace size per checkpoint.
