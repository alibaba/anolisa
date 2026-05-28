# Changelog

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
