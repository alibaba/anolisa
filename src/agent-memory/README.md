# Agent Memory

A filesystem-based persistent memory server for AI agents, implemented as an [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server in Rust. Agent Memory is a core component of [ANOLISA](../../README.md).

Think of it as a **git-backed, sandbox-isolated, full-text-searchable filesystem** that your AI agent can read, write, and query — with snapshots, audit logging, and BM25-ranked search built in.

## Features

- **19 MCP Tools** — Tier A file operations (read/write/edit/list/grep/diff), Tier B structured search & context assembly, Tier C governance (snapshots, git log/revert).
- **Filesystem Memory as MCP** — Expose a persistent, searchable, versioned filesystem to any MCP-compatible AI client.
- **FTS5 BM25 Text Index** — Background worker indexes all text files into SQLite FTS5 for sub-millisecond ranked search, with `notify`-driven incremental sync.
- **Safe Filesystem Sandbox** — Every file open routes through `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` — `..` escapes and symlink attacks are blocked at the kernel level.
- **Git Auto-Versioning** — Optional per-operation `git commit -am` so every tool call is a guaranteed revert point. `mem_log` / `mem_revert` give the model self-introspection.
- **Snapshot & Restore** — Create point-in-time tar.gz archives of the memory store; restore any snapshot atomically via per-file rename exchanges.
- **Session Isolation** — Per-process tmpfs scratch area with `mem_promote` to promote curated content into the persistent store.
- **Intelligence Profiles** — Three profiles (`basic` / `advanced` / `expert`) gate tool visibility: expert models get raw file tools only, weaker models get structured `memory_search` / `memory_observe` helpers.
- **Linux User-Namespace Isolation** — Optional `CLONE_NEWUSER` + recursive bind-mount sandboxing.
- **cgroup v2 Memory Quota** — Optional memory limit applied before the tokio runtime starts.
- **JSONL Audit Log** — Every tool call is logged to a durable JSONL audit log with optional `journald` fan-out.
- **systemd Integration** — Graceful `SIGTERM` handling with session cleanup.

## Platform

**Linux only.** Requires kernel ≥ 5.6 (`openat2` + `RESOLVE_BENEATH`), and the implementation uses user namespaces, `mount(2)`, cgroup v2, `inotify`, and `journald`. There is no macOS or Windows path.

For development on a non-Linux host, push the branch and SSH into a Linux box to build and test.

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                   MCP Client (AI Agent)               │
│                       stdio (JSON-RPC)                │
└───────────────────────┬──────────────────────────────┘
                        │
┌───────────────────────┴──────────────────────────────┐
│                 MemoryMcpServer (tools.rs)             │
│    ┌─────────┬─────────┬──────────┬──────────────┐   │
│    │ Tier A  │ Tier B  │ Tier C   │ Session      │   │
│    │ 10 tools│ 3 tools │ 5 tools  │ 1 tool       │   │
│    └────┬────┴────┬────┴────┬─────┴──────┬───────┘   │
│         │         │         │            │            │
│         └─────────┴────┬────┴────────────┘            │
│                        │                              │
│              ┌─────────┴─────────┐                    │
│              │  MemoryService    │                    │
│              │  (facade/router)  │                    │
│              └───┬───────┬───────┘                    │
│                  │       │                            │
│    ┌─────────────┼───────┼──────────────────┐        │
│    │             │       │                  │        │
│    ▼             ▼       ▼                  ▼        │
│ ┌────────┐ ┌──────────┐ ┌────────┐ ┌─────────────┐  │
│ │safe_fs │ │  index/  │ │ git_   │ │  snapshot/  │  │
│ │openat2 │ │  worker  │ │  repo  │ │    tar      │  │
│ │sandbox │ │  (FTS5)  │ │        │ │             │  │
│ └────────┘ └──────────┘ └────────┘ └─────────────┘  │
│                                                      │
│   Mount Root: ~/.anolisa/memory/<ns>/                │
│   .anolisa/                                          │
│   ├── audit.log         (JSONL, one line per call)   │
│   ├── index/bm25.db     (SQLite FTS5)                │
│   └── snapshots/        (tar.gz archives)            │
└──────────────────────────────────────────────────────┘
```

## Quick Start

### Prerequisites

- Linux (kernel ≥ 5.6)
- Rust ≥ 1.85
- `cmake` + `libsystemd` headers

### Build from Source

```bash
# From the anolisa repo root
cd src/agent-memory
cargo build --release

# Or use the unified build script
./scripts/build-all.sh --component memory
```

### Run as MCP Server

```bash
./target/release/agent-memory serve
```

The server speaks MCP JSON-RPC over stdio. Configure your MCP client to spawn it as a child process.

### Configuration

Create `~/.anolisa/memory.toml` (optional — defaults work out of the box):

```toml
[global]
user_id = "my-agent"    # defaults to OS uid

[memory]
profile = "advanced"    # basic | advanced | expert

[memory.paths]
base_dir = "~/.anolisa/memory"

[memory.session]
base_dir = "/run/anolisa/sessions"
end_action = "discard"  # discard | keep

[memory.index]
enabled = true

[memory.mount]
strategy = "auto"       # auto | userland | userns

[memory.audit]
journald = false

[memory.git]
enabled = false
auto_commit = true

[memory.cgroup]
enabled = false
memory_max = "256M"
```

#### Environment Variable Overrides

| Variable | Effect |
|----------|--------|
| `USER_ID` | Override user identifier |
| `MEMORY_BASE_DIR` | Override mount base directory |
| `MEMORY_PROFILE` | Override intelligence profile |
| `MEMORY_SESSION_DIR` | Override session scratch directory |
| `MEMORY_SESSION_END` | Override session end action |
| `MEMORY_INDEX_ENABLED` | Enable/disable FTS5 index |
| `MEMORY_MOUNT_STRATEGY` | Override mount strategy |
| `MEMORY_AUDIT_JOURNALD` | Enable/disable journald mirroring |
| `MEMORY_GIT_ENABLED` | Enable/disable git versioning |
| `MEMORY_GIT_AUTO_COMMIT` | Enable/disable auto-commit |
| `MEMORY_CGROUP_ENABLED` | Enable cgroup v2 quota |
| `MEMORY_CGROUP_MEMORY_MAX` | Memory limit (e.g. "256M") |
| `MEMORY_MAX_READ_BYTES` | Max single read (default 1 MiB) |
| `MEMORY_MAX_WRITE_BYTES` | Max single write (default 16 MiB) |
| `MEMORY_MAX_APPEND_BYTES` | Max single append (default 4 MiB) |
| `MCP_CLIENT_NAME` | Agent name recorded in session log |

### CLI Commands

```bash
agent-memory serve    # Start as MCP server (default)
agent-memory init     # Initialize namespace mount
agent-memory info     # Print resolved configuration
```

## MCP Tools Reference

### Tier A — File Operations (10 tools)

These are the core CRUD primitives. All paths are relative to the mount root.

| Tool | Parameters | Description |
|------|-----------|-------------|
| `mem_read` | `path: string` | Read a UTF-8 text file. Returns full contents. Capped at max_read_bytes (default 1 MiB). |
| `mem_write` | `path: string`, `content: string`, `overwrite?: bool` | Write a UTF-8 text file. Creates parent dirs. `overwrite=false` fails if file exists. |
| `mem_append` | `path: string`, `content: string` | Append UTF-8 text (creates if missing). Capped per call; total file size unbounded. |
| `mem_edit` | `path: string`, `old_str: string`, `new_str: string` | Replace exactly one occurrence. Errors on zero or multiple matches. |
| `mem_list` | `dir?: string`, `recursive?: bool`, `glob?: string` | List directory entries. `recursive` max depth 16. `glob` filters by pattern (e.g. `**/*.md`). |
| `mem_grep` | `pattern: string`, `dir?: string`, `type?: string`, `max?: u32`, `case_insensitive?: bool` | Regex search across files. Returns `[{path, line, text}]`. `type` is a glob filter. |
| `mem_diff` | `path1: string`, `path2: string` | Unified diff between two files in the store. |
| `mem_mkdir` | `path: string` | Create directory (with parents). Idempotent. |
| `mem_remove` | `path: string`, `recursive?: bool` | Remove file or directory. `recursive=true` required for non-empty dirs. |
| `mem_promote` | `session_path: string`, `store_path: string` | Copy a file from session scratch to persistent store. Destination must not exist. |

### Session Introspection (1 tool)

| Tool | Parameters | Description |
|------|-----------|-------------|
| `mem_session_log` | _(none)_ | Read this session's running JSONL tool-call log. Empty log returns `"(session log is empty)"`. |

### Tier B — Structured Search & Context (3 tools)

Designed for weaker models that benefit from structured APIs. Hidden on `expert` profile.

| Tool | Parameters | Description |
|------|-----------|-------------|
| `memory_search` | `query: string`, `top_k?: u32` | BM25 search across the indexed memory store. Returns ranked `[{path, snippet, score}]`. |
| `memory_observe` | `content: string`, `hint?: string` | Record an observation as `notes/observed/<ulid>.md` with frontmatter. Returns the relative path. |
| `memory_get_context` | `max_tokens?: u32` | Assemble a token-bounded context from most recently modified files. Returns markdown previews. |

### Tier C — Governance (5 tools)

Snapshot & git versioning tools for state management.

| Tool | Parameters | Description |
|------|-----------|-------------|
| `mem_snapshot` | `name?: string` | Create a point-in-time tar.gz archive. Returns `{id, name, created_at, size, backend}`. |
| `mem_snapshot_list` | _(none)_ | List all snapshots, oldest → newest. |
| `mem_snapshot_restore` | `id: string` | Restore a snapshot by id. Uses per-file rename — crash-safe: either old or new state per path. |
| `mem_log` | `limit?: u32`, `path?: string` | List recent git commits. `path` filters to commits touching that file. Errors if git is disabled. |
| `mem_revert` | `path: string` | Revert a single file to its HEAD content, then commit. Errors if git is disabled. |

### Intelligence Profiles

Tools are gated by profile — both at `tools/list` time (hidden) and `tools/call` time (hard-rejected with `METHOD_NOT_FOUND`):

| Profile | Target Models | Tier A | Tier B | Tier C |
|---------|--------------|--------|--------|--------|
| `basic` | Weak models | ✅ | ✅ | ✅ |
| `advanced` | Strong models (default) | ✅ | ✅ | ✅ |
| `expert` | Frontier models | ✅ | ❌ hidden | ✅ |

## Security Model

Agent Memory is designed to be used by AI agents — programs that are, by definition, not fully trusted. The security model assumes the agent may attempt to read or write outside its designated sandbox.

### Kernel-Level Sandboxing (`safe_fs`)

Every file open routes through `openat2(2)` with two critical flags:

- **`RESOLVE_BENEATH`** — The kernel refuses to resolve `..` beyond the mount root. Path traversal is blocked at the syscall level.
- **`RESOLVE_NO_SYMLINKS`** — No symlink on the resolution path is followed. An attacker who plants a symlink `notes/x → ~/.ssh/id_rsa` cannot trick `mem_read("notes/x")` into reading the target.

The mount root is opened once at startup with `O_PATH`, and all subsequent file operations use this `root_fd` as the `dirfd` for `openat2`. The kernel enforces the sandbox — there is no userspace path-scanning race.

### Safe Directory Removal

`std::fs::remove_dir_all` follows symlinks — a symlink inside the target dir pointing outside would destroy the link target. Agent Memory implements its own `remove_dir_all_safe` that:
1. Opens the directory via `openat2` (BENEATH + NO_SYMLINKS)
2. Enumerates entries via `fdopendir` (anchored to a kernel fd, not a path)
3. Classifies each entry with `fstatat(parent_fd, name, AT_SYMLINK_NOFOLLOW)`
4. Rejects symlinks (`S_IFLNK`) with `PathOutsideMount`
5. Removes files via `unlinkat` (by parent-fd + name, no path re-resolution)

### User Namespace Isolation (`mount`)

When `strategy = "userns"` (or `auto` on Linux), the process enters a user namespace via `unshare(CLONE_NEWUSER)` and performs a recursive bind-mount of the memory store. This adds a second layer of protection above `openat2`.

### cgroup v2 Memory Quota

When enabled, the process joins a cgroup v2 with a hard `memory.max` before the tokio runtime spawns. This prevents a runaway agent from exhausting system memory.

## Git Auto-Versioning

When `[memory.git].enabled = true`:

1. On startup, the mount root is initialized as a git repo (idempotent). A `.gitignore` excludes `.anolisa/`.
2. When `auto_commit = true`, every successful write-side tool call triggers a best-effort `git commit -am`.
3. Empty commits (unchanged tree) are silently skipped — writing identical content won't pollute the log.
4. A `commit_mutex` serializes all repo access so concurrent MCP tool calls never race on git's `index.lock`.
5. Git operations (init, commit, log, revert) are synchronous blocking I/O measured at sub-100ms on ext4 for typical mounts.

## FTS5 BM25 Index

The index worker runs in a background thread:

1. **Startup**: Performs a full scan of the mount tree, indexing every text file.
2. **Runtime**: Uses `notify` (inotify) to watch the mount tree and syncs incrementally on write/remove events.
3. **Storage**: SQLite FTS5 with trigram tokenizer, supporting BM25 ranking via the built-in `bm25()` function.
4. **Schema**: `files` table (path, mtime, size, indexed_at) + `files_fts` virtual table (path UNINDEXED, body).
5. **Upserts** are transactional — can never leave `files` and `files_fts` out of sync on crash.
6. **Directory removal** cascades to all child paths to prevent stale FTS hits.
7. **Schema versioning**: On-disk DB has a `user_version`; downgrade to older binary is refused.

Memory Service degrades gracefully when the index is unavailable — `memory_search` and `memory_observe` will return errors, but all Tier A file operations continue to work.

## Project Structure

```
src/agent-memory/
├── Cargo.toml
├── examples/
│   └── mcp_harness.rs       # Standalone MCP test harness
├── src/
│   ├── main.rs              # CLI entry point (serve/init/info)
│   ├── lib.rs               # Crate root, module declarations
│   ├── config.rs            # TOML config + env var overrides
│   ├── error.rs             # Error types
│   ├── service.rs           # MemoryService — top-level facade
│   ├── safe_fs/             # Kernel-level filesystem sandbox
│   │   └── mod.rs           # openat2(RESOLVE_BENEATH|NO_SYMLINKS)
│   ├── tools/               # Per-tool implementations
│   │   ├── mod.rs
│   │   ├── read.rs / write.rs / append.rs / edit.rs
│   │   ├── list.rs / grep.rs / diff.rs
│   │   ├── mkdir.rs / remove.rs / promote.rs
│   │   ├── memory_search.rs / memory_observe.rs / memory_get_context.rs
│   │   ├── mem_snapshot.rs / mem_snapshot_list.rs / mem_snapshot_restore.rs
│   │   ├── mem_log.rs / mem_revert.rs
│   │   └── session_log.rs
│   ├── mcp_server/
│   │   ├── mod.rs           # MCP server scaffolding
│   │   └── tools.rs         # 19 MCP tool definitions + profile gating
│   ├── index/               # Background FTS5 BM25 index
│   │   ├── mod.rs           # IndexHandle (owns worker + store)
│   │   ├── store.rs         # BM25Store (SQLite FTS5 wrapper)
│   │   ├── worker.rs        # notify-driven incremental sync
│   │   └── extractor.rs     # Text extraction from files
│   ├── git_repo/
│   │   └── mod.rs           # Git versioning (init, commit, log, revert)
│   ├── snapshot/
│   │   ├── mod.rs           # Snapshot create/list/restore
│   │   └── tar.rs           # tar.gz backend
│   ├── mount/
│   │   ├── mod.rs           # Mount strategy selection
│   │   ├── linux_userns.rs  # CLONE_NEWUSER + recursive bind-mount
│   │   └── userland.rs      # Plain filesystem (no namespace)
│   ├── session/
│   │   ├── mod.rs           # Session scratch + log service
│   │   ├── id.rs            # ULID session id generation
│   │   ├── paths.rs         # Path resolution
│   │   └── service.rs       # Session log I/O
│   ├── ns/
│   │   ├── mod.rs           # Namespace and mount point
│   │   └── paths.rs         # Path validation (.. escape prevention)
│   ├── audit/
│   │   ├── mod.rs           # JSONL audit logger
│   │   └── journald.rs      # systemd-journald integration
│   └── cgroup/
│       └── mod.rs           # cgroup v2 memory limit
└── tests/                   # Integration + unit tests
    ├── e2e_agent_test.rs
    ├── file_tools_test.rs
    ├── mcp_integration_test.rs
    ├── git_test.rs
    ├── snapshot_test.rs
    ├── session_test.rs
    ├── tier_b_test.rs
    ├── profile_test.rs
    ├── cgroup_test.rs
    ├── audit_journald_test.rs
    ├── mount_strategy_test.rs
    ├── linux_userns_test.rs
    └── common/mod.rs
```

## License

Apache-2.0. See [LICENSE](../../LICENSE) for details.
