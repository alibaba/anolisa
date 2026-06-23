# SkillFS

**English** | [中文](README_zh.md)

A FUSE-backed virtual filesystem for local agent skills. SkillFS parses
`SKILL.md` files, organises skills into views, and exposes the compiled
`SKILL.md` content through a FUSE mount.

[![Rust](https://img.shields.io/badge/Rust-1.86+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

## Features

- Parses the standard `SKILL.md` schema.
- Loads both flat and categorised skill directories.
- Manages the default view and secondary views via `skillfs-views.toml`.
- Surfaces primary-view skills inside the FUSE mount.
- Always exposes `skill-discover`, which lists secondary-view skills and
  their source paths.
- Runs conditional compilation and command normalisation when `SKILL.md`
  is read.
- Passes physical files and subdirectories inside a skill directory
  straight through to the host filesystem.
- Supports both normal mount and in-place mount.
- Supports write passthrough after mount, and syncs `SKILL.md` changes
  back into the store.
- Provides a Linux POSIX compatibility baseline for ordinary
  passthrough paths, including fd-backed I/O, create/mkdir mode
  handling, long-path fallbacks, post-unlink handles, safe symlink and
  hardlink policy, FIFO creation, and conservative `user.*` xattr
  passthrough.

## Feature Matrix

| Operation | normal mount | in-place mount | Notes |
|-----------|--------------|----------------|-------|
| `readdir` | virtual view | virtual view | visibility decided by views + store |
| read `SKILL.md` | compiled | compiled | goes through `compiler::compile` |
| read other files | passthrough | passthrough | served directly from the physical file |
| write `SKILL.md` | passthrough + store reparse | passthrough + store reparse | directory name is the authoritative store key |
| `create` regular file | passthrough | passthrough | does not trigger store update |
| `mkdir` skill dir | immediately visible | immediately visible | a degraded placeholder is inserted first |
| `rename` skill dir | visibility switches instantly | visibility switches instantly | no gap; the old name is removed at once |
| `unlink` `SKILL.md` | removed from store | removed from store | skill disappears from the virtual view |
| `rmdir` skill dir | removed from store | removed from store | inode mappings are cleaned up recursively |
| `setattr(size)` | truncate supported | truncate supported | other control attributes are not a focus |
| `symlink` | restricted passthrough | restricted passthrough | relative same-skill targets only |
| `link` | restricted passthrough | restricted passthrough | regular files inside the same skill |
| `mkfifo` | passthrough | passthrough | FIFO only; device nodes remain rejected |
| `xattr user.*` | passthrough | passthrough | ordinary passthrough paths only |

## Scope

- The CLI entry points are `mount`, `classify`, `validate`, and `list`.
- Skill visibility is driven entirely by `skillfs-views.toml`.
- Write passthrough is enabled after mount; only `SKILL.md` changes
  trigger a store sync.
- The directory name is the authoritative key in the store; the
  frontmatter `name:` field can no longer revive a stale skill name
  after rename.

## Architecture

```text
physical skills dir
  └─ skill-name/SKILL.md
            │
            ▼
    skillfs-core
      - parser
      - store
      - views
      - compiler
            │
            ▼
      skillfs-fuse
            │
            ▼
     mounted /skills view
```

## Write Path & Consistency

SkillFS is no longer a pure read-only filesystem; it is a hybrid model
of "virtual directory view + physical write passthrough":

- `readdir` is still controlled by the virtual view.
- Reads of `SKILL.md` still return the compiled content, not the raw
  file.
- All other file I/O goes straight to the underlying filesystem.
- Writes, creates, and post-rename writes to `SKILL.md` are picked up
  by the background sync worker, which reparses and updates
  `SharedSkillStore`.
- `mkdir` / `rename` on a skill directory follow the immediate-consistency
  path: the store is updated synchronously first, then an asynchronous
  reparse replaces the placeholder with the real entry.
- in-place mount accesses the underlying source through
  `/proc/self/fd/{n}` to avoid the over-mount self-loop.

For the full consistency model and scenario tables (mount-mode
comparison, in-view vs. out-of-view comparison), see
[docs/specs/skillfs-spec.md](docs/specs/skillfs-spec.md).

## Quick Start

### Build

```bash
cargo build --release
```

### Common commands

```bash
# Validate skills
cargo run -p skillfs -- validate /path/to/skills

# List skills
cargo run -p skillfs -- list /path/to/skills

# Generate or inspect skillfs-views.toml
cargo run -p skillfs -- classify /path/to/skills

# Mount the FUSE filesystem
cargo run -p skillfs -- mount /path/to/skills /path/to/mountpoint
```

### `skillfs-views.toml`

Skill selection is driven by `skillfs-views.toml`:

```toml
[[view]]
name = "major"
default = true
description = "Skills shown directly in /skills"
skills = ["github", "notion", "slack"]

[[view]]
name = "other"
default = false
description = "Skills exposed via skill-discover"
skills = ["apple-notes", "blogwatcher"]
```

After mount:

- `/skills` shows the skills assigned to the default view.
- `skill-discover/SKILL.md` enumerates the secondary-view skills and
  their `source_path`.

## `SKILL.md` Format

```markdown
---
name: my-skill
description: Brief description
version: 1.0.0
tags: [tooling, example]
enabled: true
---

# My Skill

Detailed instructions.

## Parameters

- `input` (string, required): Input value
- `options` (object, optional): Extra options

## Returns

- `result` (string, required): Result value
```

## Conditional Compilation

When the FUSE layer reads a `SKILL.md`, it invokes `compiler::compile`,
which supports:

- `<!-- @if os == darwin -->`
- `<!-- @if has_command("uv") -->`
- `<!-- @else -->`
- `<!-- @endif -->`

Even when no conditional blocks are present, the compiler performs a
small amount of heuristic command normalisation, for example:

- `pip install` → `uv pip install`
- `python -m venv` → `uv venv`
- `npm install` → `pnpm install` / `yarn install`

## Project Layout

```text
crates/
  skillfs-core/   parser, store, views, compiler, env, watcher
  skillfs-fuse/   FUSE filesystem and POSIX passthrough layer
  skillfs-cli/    mount / classify / validate / list
docs/specs/       implementation specifications
docs/testing/     POSIX acceptance and external harness docs
scripts/          build.sh, test.sh, and optional POSIX harness
```

## Test Scripts

- [scripts/build.sh](scripts/build.sh) — builds the whole workspace.
- [scripts/test.sh](scripts/test.sh) — end-to-end mount test that:
  - creates a temporary skill source directory and `skillfs-views.toml`,
  - verifies the FUSE mount succeeds,
  - verifies `/skills` exposes the default-view skills,
  - verifies `skill-discover` correctly lists the secondary view and
    each skill's `source_path`,
  - verifies physical-file passthrough inside a skill directory,
  - verifies `SIGTERM` unmounts cleanly.

## Test Coverage

`crates/skillfs-fuse/tests/write_guard_tests.rs` currently covers:

- normal mount: `SKILL.md` reads, write passthrough, immediate
  visibility of `mkdir`, no-gap `rename`, post-rename stale frontmatter
  does not revive the old name.
- in-place mount: immediate visibility of `mkdir`, no-gap `rename`,
  post-rename stale frontmatter does not revive the old name.
- Metadata guard rails around `.skill-meta/**` and virtual paths.

Additional FUSE integration suites cover POSIX passthrough behavior:

- open/read/write flag handling and fd-backed I/O;
- create/mkdir mode and umask behavior;
- PATH_MAX fallback behavior and open-after-unlink handles;
- same-skill symlink and hardlink policy;
- FIFO creation and device-node rejection;
- `user.*` xattr get/list/set/remove behavior.

The optional pjdfstest harness lives in
[scripts/posix/run_pjdfstest.sh](scripts/posix/run_pjdfstest.sh), with
operator docs in
[docs/testing/posix-external-harness.md](docs/testing/posix-external-harness.md).

`skillfs-core` covers parser, store, and watcher with both unit and
integration tests.

## Highlights

- Virtual view is decoupled from the physical filesystem: directory
  listings come from views + store, while file content still comes from
  the real source.
- Read/write split on `SKILL.md`: reads serve the compiled output to the
  agent, writes land on the raw file on disk.
- The directory name is the single authoritative key after rename, so
  stale frontmatter cannot reinsert the old skill name into the store.
- in-place mount uses a directory fd to bypass FUSE itself, avoiding
  the self-loop on writeback.

## Documentation

- [docs/specs/skillfs-spec.md](docs/specs/skillfs-spec.md) — overall
  architecture, runtime consistency boundaries, scenario comparison.
- [docs/specs/core-spec.md](docs/specs/core-spec.md) — `skillfs-core`
  implementation.
- [docs/specs/fuse-spec.md](docs/specs/fuse-spec.md) — `skillfs-fuse`
  implementation.
- [docs/specs/posix-phase1-spec.md](docs/specs/posix-phase1-spec.md) —
  POSIX passthrough baseline.
- [docs/testing/posix-phase1-acceptance.md](docs/testing/posix-phase1-acceptance.md) —
  acceptance checklist.
- [POSIX_FS_TEST_MATRIX.csv](POSIX_FS_TEST_MATRIX.csv) — POSIX test
  matrix and current coverage.

## Verification

The commands below are the CI-equivalent checks. Run them locally
before opening a pull request to keep the feedback loop short; a
change that fails any of them is not ready to merge. Formatting and
clippy are mandatory before committing SkillFS code changes.

```bash
# 1. Formatting — must produce no diff.
cargo fmt --all --check

# 2. Clippy — must finish with zero warnings under -D warnings.
cargo clippy --workspace --all-targets -- -D warnings

# 3. Unit and integration tests across the workspace.
cargo test --workspace

# 4. End-to-end FUSE mount test (requires fuse3 + /dev/fuse;
#    the script skips itself cleanly on macOS or in containers
#    without /dev/fuse).
scripts/test.sh

# 5. Rustdoc — required when public API or doc comments change;
#    recommended otherwise. Catches broken intra-doc links.
cargo doc --workspace --no-deps
```

See [AGENTS.md](AGENTS.md) for the full contributor playbook
(commenting style, module layout, dependency policy, error handling,
commit conventions).
