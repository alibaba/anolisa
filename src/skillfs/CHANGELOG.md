# Changelog

All notable changes to SkillFS are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1] - 2026-07-03

### Added
- Managed mount supervision can recover stale FUSE mounts and bound recovery
  retries during repeated starts.

### Changed
- English and Chinese README guidance now covers managed mounts, in-place
  operation, security boundaries, and troubleshooting.

### Fixed
- Post-publish grace reads fallback skill files from source paths after
  installers finish.
- `skillfs validate` now reports parse failures in the status summary.
- In-place authoring supports new skills and pending-install ownership changes.
- Managed stop and runtime-dir handling avoid stale ownership and unbounded
  recovery retries.
- Daemon-facing backing roots under PrivateTmp are rejected before mount
  startup.
- FUSE smoke cleanup handles leftover mounts and temporary paths more reliably.

## [0.3.0] - 2026-06-26

### Added
- Runtime security integration for agent skill directories. SkillFS can now
  consume activation decisions from `.skill-meta/activation.json` or the
  `user.agent_sec.skill_ledger.activation` xattr, then expose each skill as
  current, hidden, or a trusted fallback snapshot.
- File-change notification for external security daemons. With
  `--activation-mode file`, `--notify-socket`, `--activation-events-log`, and
  `--activation-reload-mode poll`, SkillFS reports skill mutations, reloads
  activation decisions, and keeps already-opened file handles pinned to their
  original target.
- Trusted control socket for activation writes. A daemon verified with
  `SO_PEERCRED`, executable identity, and start-time checks can update
  activation JSON or activation xattr through a bounded request API instead of
  writing `.skill-meta` through the agent-visible mount path.
- Installer compatibility for common skill installation flows. Staging
  directories, direct writes, quiet-timeout completion, and post-publish grace
  windows allow installers to finish writing a skill before SkillFS asks the
  security provider to scan and activate it.
- In-place mount support for security daemons. Ledger backing roots are bind
  mounted privately and validated at startup so scanners read the real source
  tree rather than the agent-facing FUSE view.
- Canonical skill identity based on the directory basename. Frontmatter
  `name:` remains display metadata and no longer changes the SkillFS store key
  or daemon-facing skill id.

### Changed
- `.skill-meta/**` is hidden from ordinary agents and remains accessible only
  through trusted metadata paths or the control socket.
- Skill mutation notify uses ordinary filesystem event kinds, including
  `create`, `write`, `rename`, `unlink`, `rmdir`, and truncate events, instead
  of a separate install-complete protocol event.
- POSIX passthrough behavior was expanded for symlink, hardlink, FIFO, path
  length fallback, open-after-unlink, xattr, and inode consistency cases.

### Fixed
- Prevented stale activation views by combining notify-triggered reload,
  polling, and activation watcher convergence.
- Hardened trusted-writer and trusted-peer checks against process reuse and
  executable replacement with start-time and file-identity validation.
- Avoided installer and daemon visibility bugs around hidden skills, fallback
  snapshots, staging paths, and backing-root propagation.

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
