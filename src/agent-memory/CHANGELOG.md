# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Prompt-injection safety module (`looksLikePromptInjection` + `escapeMemoryForPrompt`)
  with 8 heuristics mirrored between Rust core and TypeScript adapter.
- `SearchHit.suspicious` field automatically populated from full-body injection check.
- `<relevant-memories>` safety wrapper for all memory content injected into LLM prompts.
- Auto-recall: `before_prompt_build` hook injects relevant memories each turn.
- Auto-capture: `agent_end` hook with trigger-based filtering, SHA256 dedup, and
  injection rejection before persisting observations.
- Dense-vector semantic search via pluggable `EmbeddingProvider` trait:
  OpenAI (`/v1/embeddings`) and Ollama (`/api/embed`) backends.
- `files_vec` table (schema v2) for per-file dense embeddings alongside FTS5 BM25.
- Hybrid search with reciprocal rank fusion (RRF, k=60) of BM25 + vector scores.
- `memory_search` `mode` parameter: `bm25` (default), `vector`, `hybrid`.
- Graceful fallback from vector/hybrid to BM25 when no embedding provider is configured.
- Embedding computed automatically during index worker flush (phase 2).
- System prompt integration (`promptBuilder`) with tool usage guidelines.
- Corpus supplement registration for `memory_search corpus=all` integration.
- `EmbeddingConfig` (None|OpenAI|Ollama) with TOML parsing and environment variable overrides.
- 30-second HTTP timeout on embedding API clients.

### Changed

- `memory_search` signature extended with optional `mode` parameter (backward-compatible).
- Index handle (`IndexHandle`) now carries an optional embedding provider reference.
- `reqwest` dependency added with `rustls-tls` and `json` features.

### Fixed

- `effectiveMode` in search tool response now reflects the actual mode used.
- Embedding API empty-response handling: returns zero vector of correct dimensionality
  instead of dimension-0 vector.

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
- OpenClaw plugin `memory-anolisa` bundled under `/usr/share/anolisa/adapters/agent-memory/openclaw/`: 4 OpenClaw memory contract tools (`memory_search` / `memory_get` / `memory_observe` / `memory_get_context`) routed to the agent-memory MCP server as a stdio child with lazy start, bounded respawn (3 attempts), per-method timeouts, env allowlist, and a bounded stderr ring buffer. `install.sh` / `uninstall.sh` register/clean via the OpenClaw CLI; RPM `%preun` auto-cleans `plugins.{allow,entries,slots}` from `openclaw.json`.
- Single-source version sync: `Cargo.toml` is the authority, Makefile `sync-versions` propagates the value (via `jq`, idempotent) into `manifest.json` / `package.json` / `package-lock.json` / `openclaw.plugin.json` / `mcp-server.json`, and esbuild `--define` injects the same constant into the bundle's `PLUGIN_VERSION` — so the RPM header, binary, plugin manifest, and MCP `initialize.clientInfo.version` always agree.
- Interactive `mcp-harness` example for manual tool-call verification; 140 automated Rust tests across 12 integration suites plus lib/main unit tests covering all 19 tools, plus TypeScript unit tests for the OpenClaw plugin's config validation and tool-name mapping.
