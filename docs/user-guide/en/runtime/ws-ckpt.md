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

The global config file is read by the daemon, so `config -g` does more than write it: after saving `/etc/ws-ckpt/config.toml` it asks the daemon to reload, then compares what the daemon actually loaded against what was written. On any mismatch the command lists each differing field and exits non-zero instead of reporting success.

This matters most in a Kubernetes sidecar deployment, where the CLI (app container) and the daemon run in separate containers with separate filesystems. Mount `/etc/ws-ckpt` on a volume shared by both containers (an `emptyDir` is enough); otherwise every `config -g` setting silently stays at the daemon's built-in default. The bundled `k8s-sidecar-example.yaml` already wires this shared volume up.

---

## Important Notes

> **WARNING**: The workspace path configured for ws-ckpt must NOT be:
> - The root path (`/`)
> - Inside the daemon's mount_path
> - An active mount point (see below)
> - The Agent startup directory or any parent directory (validated at plugin level)
>
> These constraints are enforced by the daemon. Attempts to use invalid paths will be rejected.

### The workspace root cannot be a mount point

Initializing a workspace moves the original directory aside as a backup, and
`rename(2)` fails with `EBUSY` on a directory that is itself a mount point. Any
filesystem type is affected, not just FUSE.

The common case is an in-place SkillFS mount, where the source and the mountpoint
are the same directory. Unmount it first:

```bash
skillfs stop /path/to/workspace      # in-place SkillFS mount
fusermount3 -u /path/to/workspace    # any other FUSE mount
```

This applies to `init` and to the first `checkpoint` on an unmanaged path, which
auto-initializes. Once a workspace is initialized, later `checkpoint`, `rollback`,
`list`, and `diff` operations are unaffected.

Only the workspace root itself is rejected. A mount nested *inside* the workspace
does not block `init`, but the outcome is rarely what you want: the mount stays
attached to the backup directory that `init` moves aside, while the new workspace
receives a plain copy of the mount's contents — subsequent writes land in the
copy, not on the mounted filesystem, and the two silently diverge. Unmount nested
mounts before initializing, or keep mount points outside the workspace tree.

### Rolling back to a pre-first-conversation snapshot blocks OpenClaw

OpenClaw seeds baseline files (AGENTS.md, BOOTSTRAP.md, SOUL.md, IDENTITY.md,
and USER.md; 2026.7.x also seeds HEARTBEAT.md and TOOLS.md) into the
workspace on its first run and keeps an attestation record of that event. If
you roll back to a snapshot taken **before** OpenClaw's first conversation —
one that does not contain these baseline files — OpenClaw mistakes the
vanished files for an accidentally deleted workspace and refuses to work:

```
WorkspaceVanishedError: OpenClaw workspace appears to have disappeared ...
Refusing to reseed BOOTSTRAP.md over a recently attested workspace.
```

**Rolling back to a pre-first-conversation snapshot is strongly discouraged.**
Right after OpenClaw's first conversation in a workspace, create a snapshot
(`ws-ckpt checkpoint -w ...`); every later rollback then targets a snapshot
of a workspace that was in real use.

What the guard actually checks is survival evidence, not when the snapshot
was taken: a rollback is safe as long as the target snapshot preserves
content OpenClaw recognizes. The accepted evidence differs by version:

- All versions:
  - a still-present BOOTSTRAP.md (setup has not completed yet);
  - a profile file that differs from its template (SOUL.md, IDENTITY.md,
    USER.md);
  - the `memory/` directory (or MEMORY.md);
  - an installed skill (`skills/<name>/SKILL.md`);
  - the generated baseline files are all still present and byte-identical to
    what OpenClaw generated — their hashes are recorded in the attestation —
    so a completed workspace that was never customized also passes.
- 2026.7.x only: a required bootstrap file whose content no longer matches
  the generated one (AGENTS.md, TOOLS.md, or HEARTBEAT.md).
- 2026.8.1 and later only: a customized AGENTS.md (content differs from both
  the generated one and the template). These versions no longer seed
  TOOLS.md or HEARTBEAT.md, and neither file ever counts as evidence there.

Two consequences:

- The error triggers only when the snapshot has lost **all** of the evidence
  above — typically a state from before the baseline files were seeded, or
  one where those files were later deleted with no memory, skills, or
  customized files left behind. Merely never having customized the workspace
  does not trigger it: intact generated files are themselves evidence.
- Do not equate safety with the full seeded file list: OpenClaw deletes
  BOOTSTRAP.md once setup completes, and since 2026.8.1
  `openclaw-workspace-state.json` is only a legacy migration input — a
  normal post-conversation snapshot contains neither, yet is safe.

**To recover, pick the path for your OpenClaw version:**

- OpenClaw 2026.7.x and earlier — remove the attestation records for this
  workspace. The record file is named after the SHA-256 of the workspace's
  normalized absolute path, and OpenClaw looks for it in its state directory
  (honoring `OPENCLAW_STATE_DIR`), under the effective home directory
  (honoring `OPENCLAW_HOME`, default `$HOME`) in both the `.openclaw` and
  legacy `.clawdbot` state directories, and next to the workspace itself.
  OpenClaw normalizes paths with Node.js `path.resolve` (collapsing `..`
  segments and repeated slashes) and expands a leading `~` in its env
  overrides, so the command below runs entirely inside `node`: it derives
  the exact same paths and removes the records itself, and path content is
  never re-evaluated by the shell (Node.js is present wherever the
  npm-installed OpenClaw CLI runs).

  One input the command cannot derive by itself: an agent started as
  `openclaw --profile <name>` keeps its state under
  `<effective home>/.openclaw-<name>` (`--dev` behaves like `--profile dev`),
  and that choice exists only inside the agent process — exporting
  `OPENCLAW_PROFILE` in the shell does **not** move the state directory, only
  the command-line flag does. Rather than guess and silently clean the wrong
  directory, the command aborts when `OPENCLAW_STATE_DIR` is unset and it
  finds more than one `.openclaw*` state directory under the effective home.
  In that case, export the state directory the agent actually used and rerun:

  ```bash
  export OPENCLAW_STATE_DIR="$HOME/.openclaw-team"   # agent runs `openclaw --profile team ...`
  ```

  The recovery command:

  ```bash
  WS='/path/to/workspace'   # workspace's absolute path (single quotes keep $ and backticks literal)
  node -e '
    const crypto = require("crypto"), fs = require("fs"), os = require("os"), path = require("path");
    const env = process.env;
    if (!process.argv[1]) { console.error("Set WS to the workspace path and pass it to this command."); process.exit(1); }
    const rawHome = (env.OPENCLAW_HOME || "").trim();
    const OC_HOME = rawHome
      ? path.resolve(rawHome.replace(/^~(?=$|[\\/])/, os.homedir()))
      : os.homedir();
    const WS = path.resolve(process.argv[1]);
    const HASH = crypto.createHash("sha256").update(WS).digest("hex");
    const sdOverride = (env.OPENCLAW_STATE_DIR || "").trim();
    let SD;
    if (sdOverride) {
      SD = path.resolve(sdOverride.replace(/^~(?=$|[\\/])/, OC_HOME));
    } else {
      let candidates = [];
      try {
        candidates = fs.readdirSync(OC_HOME, { withFileTypes: true })
          .filter((e) => e.isDirectory() && (e.name === ".openclaw" || e.name.startsWith(".openclaw-")))
          .map((e) => path.join(OC_HOME, e.name));
      } catch {}
      if (candidates.length > 1) {
        console.error("Multiple OpenClaw state directories exist under " + OC_HOME + ":\n"
          + candidates.map((d) => "  " + d).join("\n") + "\n"
          + "An agent started with `openclaw --profile <name>` (or `--dev`) keeps its state in\n"
          + "<effective home>/.openclaw-<name>, and that value exists only inside the agent process.\n"
          + "Export the state directory the agent actually used, then rerun this command:\n"
          + "  export OPENCLAW_STATE_DIR=" + OC_HOME + "/.openclaw-<name>");
        process.exit(1);
      }
      SD = candidates[0] || path.join(OC_HOME, ".openclaw");
    }
    const stateDirs = [...new Set([SD, path.join(OC_HOME, ".openclaw"), path.join(OC_HOME, ".clawdbot")])];
    const targets = stateDirs.map((d) => path.join(d, "workspace-attestations", HASH + ".attested"));
    targets.push(WS + ".attested");
    console.log("workspace:   " + WS);
    console.log("state dir:   " + SD);
    let failed = false;
    for (const t of targets) {
      try {
        if (fs.existsSync(t)) { fs.rmSync(t); console.log("removed:     " + t); }
        else { console.log("not present: " + t); }
      } catch (err) {
        failed = true;
        console.error("FAILED:      " + t + " (" + err.message + ")");
      }
    }
    if (failed) process.exit(1);
  ' "$WS"
  ```

  Then rerun your agent session; OpenClaw reseeds the baseline files and
  starts a fresh attestation.

- OpenClaw 2026.8.1 and later — the attestation record has moved into
  OpenClaw's state SQLite database, and no command currently removes just
  this one record. The error message suggests a full OpenClaw reset
  (`openclaw reset --scope full`), but that deletes **every** agent workspace
  and the entire OpenClaw state directory — credentials, sessions, and
  installed plugins included — which is far more destructive than this
  situation warrants. To restore normal workspace behavior, roll back again
  to a snapshot taken while the workspace was in real use (any snapshot that
  satisfies the survival-evidence condition above):

  ```bash
  ws-ckpt rollback -w /path/to/workspace -s <snapshot-id>
  ```

  The workspace is usable again immediately. Alternatively, the guard state
  itself expires: 24 hours after the last state write, OpenClaw clears the
  stale attestation and reseeds the workspace on the next agent run, so
  keeping the pre-baseline snapshot and retrying after 24 hours also works
  without a full reset. To clear the block sooner, the full OpenClaw reset
  described above is currently the only option. Note that waiting for the
  expiry and running the reset end the same way: the next agent run reseeds
  the baseline files into the workspace.

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
