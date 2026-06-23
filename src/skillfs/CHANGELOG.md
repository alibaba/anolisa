# Changelog

All notable changes to SkillFS are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Trusted peer control socket (`--control-socket`, `--trusted-peer-exe`) with
  `SO_PEERCRED` identity verification, activation write API, and bounded
  request size (64 KB limit).
- Installer staging compatibility: `[install] staging_patterns` config for
  installer-private workspaces with rename-boundary mutation notify.
- Quiet-timeout notifications: aggregated mutation notify after a configurable
  quiet window for installers that do not use rename or `.install-complete`.
- Direct pending install transactions: track newly created skill directories
  and suppress notify until a quiet window confirms completion.
- Post-publish grace sessions: time-limited exact-path writes to whitelisted
  patterns after staging rename or pending install completion.
- Backing root propagation isolation: `MS_PRIVATE | MS_REC` after bind mount
  to prevent the FUSE over-mount from leaking into the backing root.
- Backing root accessibility validation at startup for in-place mounts with
  activation or notify enabled.
- Canonical skill identity: store key is the directory basename, not the
  frontmatter `name:` field.
- `enqueue_immediate()` on `NotifyController` for staging and quiet-timeout
  dispatch that bypasses debounce.

## [0.2.0] - 2026-05-09

### Added
- FUSE write passthrough for `write`, `create`, `mkdir`, `rename`, `unlink`,
  `rmdir`, and `setattr(size)` operations on skill directories.
- Background sync worker that reparses `SKILL.md` on write and `upsert`s the
  entry back into `SharedSkillStore`.
- Immediate visibility for newly created skill directories: `mkdir` inserts a
  `ParseStatus::Degraded` placeholder, then the sync worker overwrites it with
  the real entry once `SKILL.md` is written.
- in-place mount mode that accesses the underlying source via
  `/proc/self/fd/{n}` to avoid the over-mount self-loop.
- Integration suite `crates/skillfs-fuse/tests/write_guard_tests.rs` covering
  both normal and in-place write paths.

### Changed
- Directory name is now the authoritative store key. After `rename`, stale
  frontmatter `name:` no longer revives the old key.
- Read of `SKILL.md` still returns the compiled result; raw file is only used
  for writes and parsing.
- Architecture docs refactored into `docs/specs/skillfs-spec.md`,
  `docs/specs/core-spec.md`, `docs/specs/fuse-spec.md`.

### Removed
- Workspace-related code paths and the unused workspace config support from
  `skillfs-core` (commit 6d604c7).
- Legacy ad-hoc test scripts (kept only `scripts/build.sh` and
  `scripts/test.sh`).

### Fixed
- CLI tracing timestamps now use the local timezone instead of UTC.

## [0.1.2] - 2026-04-29

### Added
- Read-only mount write protection: `mknod`, `symlink`, `link`, and write
  callbacks all return `EROFS`.

### Fixed
- Parser summary truncation now respects multi-byte character boundaries.

## [0.1.1] - 2026-04-29

### Added
- `skillfs-mount` agent skill under `docs/skills/` to help users set up,
  mount, and unmount a SkillFS instance.

## [0.1.0] - 2026-04-25

### Added
- Initial release of the SkillFS workspace.
- `skillfs-core`: `SKILL.md` parser (with `Ok` / `Degraded` / `Error` status),
  in-memory `SkillStore` with flat and categorized directory layouts,
  `skillfs-views.toml` configuration, conditional `compiler::compile`, and
  environment probing (OS, commands, env vars).
- `skillfs-fuse`: read-only FUSE filesystem that exposes the configured
  default view at `/skills`, always-on virtual `skill-discover`, and
  compile-on-read for `SKILL.md`. Other files in a skill directory are
  passed through to the physical source.
- `skillfs` CLI: `mount`, `classify`, `validate`, `list` subcommands.
