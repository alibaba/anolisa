# SkillFS

SkillFS is a FUSE-based virtual filesystem that exposes curated agent skills through a `/skills` view, using default view + skill-discover to control which skills are visible to Agents.

## Overview

| Capability | Description |
|------------|-------------|
| View-based visibility | Default view skills appear in `/skills`; others accessible via `skill-discover` |
| Conditional compilation | `SKILL.md` is compiled on read (OS conditions, command normalization) |
| Physical write passthrough | Non-SKILL.md files pass through to the underlying filesystem |
| skill-discover | Virtual skill that lists secondary view skills with source paths |

## Prerequisites

| Requirement | Minimum |
|-------------|---------|
| OS | Linux |
| Filesystem | FUSE3 (`libfuse3-dev` or equivalent) |
| Device | `/dev/fuse` must be available |
| Rust | >= 1.86 (source build only) |

## Installation

```bash
# Recommended
anolisa install skillfs

# Source build (developers only)
cd src/skillfs && cargo build --release
```

## Quick Start

```bash
# Validate skills in a source directory
skillfs validate /path/to/skills

# List all skills
skillfs list /path/to/skills

# Generate skillfs-views.toml (assigns skills to default + secondary views)
skillfs classify /path/to/skills

# Mount the virtual filesystem
skillfs mount /path/to/skills /mnt/skillfs --foreground
# Agent accesses: /mnt/skillfs/skills/<skill-name>/SKILL.md
```

## Usage

### skillfs mount — Mount Virtual Filesystem

```bash
skillfs mount <SOURCE> <MOUNTPOINT> [OPTIONS]
```

- `SOURCE`: Directory containing skill folders (each with `SKILL.md`) and `skillfs-views.toml`
- `MOUNTPOINT`: Directory where SkillFS exposes the virtual `/skills` view

After mount, Agents access skills at:
```
<MOUNTPOINT>/skills/<skill-name>/SKILL.md
```

The `skill-discover` virtual skill is always present at:
```
<MOUNTPOINT>/skills/skill-discover/SKILL.md
```

Key options:
- `--foreground`: Keep in foreground (useful for tests and systemd)
- `--security-mode`: Require in-place mount (SOURCE = MOUNTPOINT)
- `--audit-log <PATH>`: Append filesystem audit events as JSONL
- `--pid-file <PATH>`: Write PID file for process management

Example:
```bash
# Development mount
skillfs mount ./skills /mnt/skillfs --foreground

# In-place mount with audit
skillfs mount ./skills ./skills --security-mode --audit-log /var/log/skillfs/audit.jsonl
```

### skillfs classify — Generate View Configuration

```bash
skillfs classify <SOURCE> [--primary-count N] [--dry-run]
```

Generates `skillfs-views.toml` in the source directory. The first N skills go into the default view ("major"), the rest into a secondary view ("other").

```bash
# Generate with default 6 primary skills
skillfs classify /path/to/skills

# Preview without writing
skillfs classify /path/to/skills --dry-run

# Customize primary count
skillfs classify /path/to/skills --primary-count 10
```

### skillfs validate — Validate Skill Files

```bash
skillfs validate <SOURCE> [--format text|json]
```

Validates all `SKILL.md` files in the source directory. Reports success, degraded (partial parse), and error states.

```bash
# Text output (default)
skillfs validate /path/to/skills

# JSON output (for CI integration)
skillfs validate /path/to/skills --format json
```

### skillfs list — List Skills

```bash
skillfs list <SOURCE> [--enabled-only]
```

Lists all skills found in the source directory with their metadata.

```bash
# List all skills
skillfs list /path/to/skills

# Only show enabled skills
skillfs list /path/to/skills --enabled-only
```

## Configuration

SkillFS uses `skillfs-views.toml` in the **source directory** (not a global config path) to control skill visibility:

```toml
[[view]]
name = "major"
default = true
description = "Core skills shown directly in /skills"
skills = ["github", "notion", "slack"]

[[view]]
name = "other"
default = false
description = "Additional skills accessible via skill-discover"
skills = ["apple-notes", "blogwatcher"]
```

Behavior:
- The `default = true` view's skills appear in `<mountpoint>/skills/`
- Secondary views are listed in `skill-discover/SKILL.md` with `source_path` for each skill
- Skills not assigned to any view are automatically added to the default view on next mount

## Architecture

```
crates/
  skillfs-core/   parser, store, views, compiler, env, watcher
  skillfs-fuse/   FUSE filesystem and POSIX passthrough layer
  skillfs-cli/    mount / classify / validate / list
```

## FAQ

**Q: Where does the config file go?**

A: `skillfs-views.toml` lives in the skills source directory (the same directory you pass as `<SOURCE>`), not in `~/.config/`.

**Q: What does skill-discover do?**

A: It's a virtual skill always present in `/skills`. When secondary views exist, it lists those skills with their `source_path` so Agents can use `read_file` to access them directly.

**Q: Can Agents write through the FUSE mount?**

A: Yes. Non-SKILL.md files pass through to the physical filesystem. Writing to `SKILL.md` triggers a re-parse to keep the internal store consistent.
