# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-05-27

### Added

- Initial release: filesystem memory MCP server for AI agents (Linux only).
- 19 MCP tools over stdio JSON-RPC 2.0 in three tiers:
  - Tier A file ops: `mem_read` / `mem_write` / `mem_append` / `mem_edit` / `mem_list` / `mem_grep` / `mem_diff` / `mem_mkdir` / `mem_remove` / `mem_promote` / `mem_session_log`.
  - Tier B structured search: `memory_search` (BM25) / `memory_observe` / `memory_get_context`.
  - Tier C governance: `mem_snapshot` / `mem_snapshot_list` / `mem_snapshot_restore` / `mem_log` / `mem_revert`.
- Per-namespace mount under `~/.anolisa/memory/<ns>/` with optional Linux user-namespace + private tmpfs isolation; pluggable `auto` / `userland` / `userns` strategies.
- Path sandbox via `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` on every Tier A file open; `fdopendir` + `fstatat` + `unlinkat` for recursive removal so symlink swaps cannot race.
- SQLite FTS5 BM25 background index with transactional upsert, schema-versioned migrations, trigram tokenizer for CJK, inotify-driven debounced flush, and full rescan on overflow.
- Optional git versioning with auto-commit serialized under a per-handle mutex; commits offloaded via `tokio::task::spawn_blocking`; empty trees skipped.
- tar.gz snapshots with strict id whitelist, atomic per-entry rename swap on restore, and rollback entries preserved under `.anolisa/trash/` instead of deleted.
- Optional cgroup v2 `memory.max` self-limit applied before the tokio runtime starts.
- JSONL audit log opened with `O_NOFOLLOW | O_CLOEXEC` and held as `Mutex<File>`; optional systemd-journald fan-out.
- Profile gating (`basic` / `advanced` / `expert`) enforced at both `tools/list` and `tools/call`; `deny_unknown_fields` on every config struct so misspelt keys hard-fail at load.
- Per-session scratch and log under `/run/anolisa/sessions/<sid>/` with `0700` permissions; tmpfiles.d snippet ships the directory.
- systemd user template `anolisa-memory@.service` with hardening (`ProtectKernelTunables/Modules/Logs`, `SystemCallFilter=@system-service`, `MemoryDenyWriteExecute`, `RestrictNamespaces` allowlist `user mnt`, `RestrictAddressFamilies=AF_UNIX`).
- RPM packaging with offline vendor tarball (`Source1`); single statically-linked binary (bundled SQLite + vendored libgit2).
- Interactive `mcp-harness` example for manual tool-call verification; 140 automated tests across 12 integration suites plus lib/main unit tests covering all 19 tools.
