# Changelog

## 0.4.0

### Breaking Changes
- **BREAKING** checkpoint `-i`/`--id` flag replaced by `-s`/`--snapshot` as primary; `-i` remains as hidden alias but may be removed in a future release (#1064)

### Features
- Added plugin install/uninstall subcommand (#1005)
- Added component.toml for anolisa-cli adapter discovery (#1005)
- Added rollback preview support with --preview parameter (#1103)
- Added elapsed time display after each CLI operation (#1075)
- Added auto-generated snapshot ID when --snapshot is omitted (#1064)
- Added SLS ops log output for dashboard metrics (#1059)
- Added optional -t flag for diff to compare snapshot against current workspace (#848)
- Added rollback-by-ancestor-count and snapshot DAG tracking (#877)
- Added cron-based scheduled checkpoint snapshots (#819)

### Bug Fixes
- Fixed --snapshot/-s as primary flag and aligned plugin flag handling (#1103, #1064)
- Fixed SKILL.md to sync with actual CLI/plugin implementation (#847)
- Fixed init and recover to guard against replaced workspace symlink (#860)
- Fixed init rsync by dropping --copy-unsafe-links (#873)

## 0.3.3

### Features
- Added per-workspace policy override with hermes/openclaw plugin support (#721)
- Added `/proc` cwd occupant guard for init and rollback (#684)
- Added Hermes adapter runner script (#617)

### Bug Fixes
- Fixed write lock contention and cwd guard deadlock in rollback (#721, #684)
- Fixed input validation for non-UTF-8 paths and path-traversal snapshot IDs (#695, #678)
- Fixed seccomp arch selection, workspace registry concurrency, and RPM packaging (#695, #684)

## 0.3.2

- Fixed openclaw uninstall to remove tool whitelist from config
- Fixed parent path refusal to apply as workspace-level rules for skill and openclaw plugin

## 0.3.1

- Fixed plugin workspace config registration and auto-loading
- Reject workspace paths that are hermes cwd itself or parent
- Fixed plugin tool to prefer explicit workspace parameter over config
- Fixed skill delete requiring --force flag
- Fixed daemon workspace path validation and fswatch fd leak
- Removed unused btrfs_ops.rs module

## 0.3.0

- Added openclaw plugin scaffolding for ws-ckpt
- Added hermes plugin scaffolding for ws-ckpt
- Made ws-ckpt skill agent-agnostic and prompted for workspace at invocation
- Followed `make install` contract for build-all integration
- Fixed bugs in list and diff sub-commands
- Made daemon stateful

## 0.2.0

- Added auto_cleanup feature and switch
- Unified config modification entry through the TOML file
- Added global CLI warning when any workspace>1000 snapshots or filesystem usage>90%
- Fixed backend detection and daemon state recovery logic
- Fixed image size configuration not taking effect after daemon restart
- Removed obsolete fs_warn_threshold_percent parameter
- Fixed config.toml to ship as a sample file

## 0.1.0

- Daemon with Unix Socket IPC and Bincode binary protocol.
- `init` / `checkpoint` / `rollback` / `delete` / `list` / `diff` / `cleanup` / `status` / `config` commands.
- Background scheduler: auto-cleanup, health check, orphan recovery.
- Multi-backend: btrfs-base / btrfs-loop / overlayfs with auto-detection.
- TOML config persistence with runtime hot-reload.
- systemd service with RPM packaging for Alinux 4.
