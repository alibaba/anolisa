# agent-memory User Manual (English)

> 中文版本见 [`user_manual.zh.md`](./user_manual.zh.md).

`agent-memory` is a Linux-only Rust [MCP](https://modelcontextprotocol.io/)
server that gives an AI agent persistent, sandboxed, file-shaped memory.
This manual covers the architecture, installation, configuration, the
19 MCP tools the server exposes, how to integrate from a client / SDK,
and how to verify a deployment.

## Table of Contents

1. [Overview](#1-overview)
2. [Architecture](#2-architecture)
3. [Installation](#3-installation)
4. [Configuration](#4-configuration)
5. [Feature Reference](#5-feature-reference)
6. [Tool API Reference](#6-tool-api-reference)
7. [SDK / Client Integration Guide](#7-sdk--client-integration-guide)
8. [Testing & Verification Guide](#8-testing--verification-guide)
9. [Troubleshooting](#9-troubleshooting)

---

## 1. Overview

### What is `agent-memory`

`agent-memory` is a single-binary MCP server that turns a directory on
the local filesystem into a structured memory store an agent can read
and write through 19 well-defined tools. Unlike a conversation-window
or vector-DB-only memory, the store is:

- **File-shaped** — the agent thinks in paths (`notes/x.md`,
  `decisions/2026-05/db-pick.md`), the same shape humans use.
- **Sandboxed** — every file open is anchored at the mount root via
  `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)`; the kernel rejects
  `..`, symlinks, and meta-directory access.
- **Versioned** — optional git auto-commit and tar.gz snapshots give
  rollback at file and at mount granularity.
- **Searchable** — a SQLite FTS5 BM25 index runs in the background so
  full-text queries are sub-millisecond.

### Who it's for

- Agent runtimes (Claude Code, Cursor, Continue, custom rmcp-based
  clients) that want a persistent scratchpad.
- Multi-agent systems where one agent's notes need to outlive its
  process and be readable by another.
- Operators who need an audit trail (`mem_log`, JSONL audit, journald)
  and recovery (`mem_revert`, `mem_snapshot_restore`).

### Threat model in one paragraph

The server treats the agent as an untrusted process that may try to
escape the mount, plant symlinks, mass-delete, or DoS via large
payloads. The kernel-level `RESOLVE_BENEATH` flag, the explicit
reserved-path set (`.anolisa`, `.git`, `.gitignore`), and per-call
size caps (`max_read_bytes`, `max_write_bytes`, `max_append_bytes`)
close the common-case attacks. Profile gating, audit logs, and
snapshots are defence-in-depth and recovery aids.

---

## 2. Architecture

### Layered diagram

```
+--------------------------------------------------------+
| MCP client (Claude Code / Cursor / custom)             |
|   stdio JSON-RPC 2.0                                   |
+----------------------------+---------------------------+
                             |
+----------------------------v---------------------------+
| MemoryMcpServer (rmcp)                                 |
|   tools/list  -> profile-filtered                      |
|   tools/call  -> profile-gated, returns Result<>       |
+----------------------------+---------------------------+
                             |
+----------------------------v---------------------------+
| MemoryService                                          |
|   dispatches to tool impls; owns mount, index, git,    |
|   snapshot, audit, session handles                     |
+----+------------+--------------+-----------+-----------+
     |            |              |           |
+----v---+   +----v-----+  +-----v----+  +---v-----+
| Mount  |   | Index    |  | Git repo |  | Snapshot|
| (auto/ |   | (SQLite  |  | (libgit2 |  | (tar.gz)|
|  user- |   |  FTS5)   |  |  vendored|  |         |
|  land/ |   |          |  |          |  |         |
|  userns|   |          |  |          |  |         |
+--------+   +----------+  +----------+  +---------+
     |            |              |           |
     +------+-----+--------+-----+-----+-----+
            |              |           |
+-----------v--------------v-----------v----+
| safe_fs: openat2 RESOLVE_BENEATH | NO_SYM |
|          fdopendir + fstatat + unlinkat   |
+--------------------+----------------------+
                     |
+--------------------v----------------------+
| Per-namespace mount: ~/.anolisa/memory/<ns>/
|   user-files (notes/, decisions/, ...)    |
|   .anolisa/  (audit.log, index.db, ...)   |
+-------------------------------------------+
```

### Mount strategies

| Strategy | When | What happens |
|----------|------|---------------|
| `userland` (default) | always works | Mount is just a directory; sandbox enforced by `openat2`. |
| `userns` | Linux ≥ 4.6, kernel allows unprivileged user namespaces | At startup the process `unshare`s into a fresh user + mount namespace, overlays a private tmpfs on `/mnt`, bind-mounts the backing dir there. Host-side processes see nothing under `/mnt/memory/<ns>/`. |
| `auto` | runtime-detected | Tries `userns` first; falls back to `userland` on any error. The retry path is robust against partial mount-stage failures (the `unshare` / maps stage runs at most once; mount steps are idempotent). |

### Per-namespace layout

```
~/.anolisa/memory/user-<uid>/        # mount root
├── README.md                        # auto-generated overview
├── notes/                           # free-form agent notes
├── decisions/                       # (example user-defined dirs)
└── .anolisa/                        # OS-managed, agent cannot write
    ├── manifest.toml                # namespace metadata
    ├── audit.log                    # JSONL tool-call audit
    ├── index.db                     # FTS5 SQLite database
    ├── snapshots/                   # tar.gz archives + sidecars
    ├── trash/                       # rollback entries from restore
    └── git/                         # bare git mirror (when git enabled)
```

> Indicative layout — items under `.anolisa/` are populated lazily as
> features are exercised (e.g. `git/` only exists with
> `MEMORY_GIT_ENABLED=true`).

### Per-session layout

```
/run/anolisa/sessions/<sid>/         # tmpfs, mode 0700
├── meta.toml                        # session metadata
├── log.jsonl                        # per-session tool-call log
└── scratch/                         # session-only working files;
                                     # use mem_promote to persist
```

### Index worker

A background tokio task watches the mount via `inotify`, batches events
through a 200 ms debounce window, and applies them in a single SQLite
transaction. The tokenizer is `trigram` for CJK robustness; the schema
is versioned so a future format change can migrate cleanly. On
inotify overflow (`IN_Q_OVERFLOW`) the worker falls back to a full
rescan instead of dropping events silently.

### Audit and observability

Every successful tool call appends a line to
`<mount>/.anolisa/audit.log` and (optionally)
`/run/anolisa/sessions/<sid>/log.jsonl`. With `audit.journald=true`
each line is also fanned out to systemd-journald with structured
fields (`MESSAGE_ID`, `AGENT_MEMORY_TOOL`, ...) so operators can
filter with `journalctl`. Errors return through MCP as
`CallToolResult { isError: true }` so the client distinguishes failure
from a successful call whose payload happens to start with "failed".

---

## 3. Installation

### From RPM (recommended, AnolisOS / RHEL family)

```bash
sudo yum install agent-memory
```

The package installs:

- `/usr/bin/agent-memory` — the server binary
- `/usr/share/anolisa/agent-memory/default.toml` — default config
- `/usr/share/anolisa/mcp-servers/agent-memory.json` — MCP server
  descriptor for auto-discovery
- `/usr/lib/systemd/user/anolisa-memory@.service` — opt-in systemd
  user template
- `/usr/lib/tmpfiles.d/anolisa-memory.conf` — creates
  `/run/anolisa/{,sessions}` at boot with `0700`
- `/usr/share/doc/agent-memory/{CHANGELOG.md, user_manual.md, user_manual.zh.md}`

### From source

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa/src/agent-memory
make build         # cargo build --release --locked
sudo make install  # copies binary + config under /usr/local
```

Build requirements: Rust ≥ 1.85 (edition 2024 needs 1.85; CI pins
1.89.0 to match the rest of the monorepo's Linux Rust crates so a
single toolchain image covers them all), cmake (libgit2 vendored
build), systemd-devel (for the journald audit fan-out).

### Cross-platform development

`agent-memory` is Linux-only at runtime. On macOS / Windows use the
remote build flow:

```bash
# from src/agent-memory/
make remote-build   # push branch + ssh into a Linux dev host, cargo build
make remote-test    # same + run the test suite + clippy
```

---

## 4. Configuration

### Configuration file

Default location: `~/.anolisa/memory.toml`. Unknown fields are
rejected (`serde(deny_unknown_fields)`) so typos hard-fail at load.
A minimal config:

```toml
[global]
user_id = "alice"

[memory]
profile = "advanced"           # basic | advanced | expert
max_read_bytes = 1048576       # 1 MiB
max_write_bytes = 16777216     # 16 MiB
max_append_bytes = 4194304     # 4 MiB

[memory.paths]
base_dir = "~/.anolisa/memory"

[memory.session]
base_dir = "/run/anolisa/sessions"
end_action = "discard"         # discard | keep

[memory.mount]
strategy = "auto"              # auto | userland | userns

[memory.index]
enabled = true

[memory.audit]
journald = false

[memory.cgroup]
enabled = false
memory_max = "512M"

[memory.git]
enabled = false
auto_commit = true
```

### Environment overrides

Every config field has an `MEMORY_*` env override; useful for tests
and one-off invocations.

| Env var | Equivalent | Notes |
|---------|------------|-------|
| `USER_ID` | `global.user_id` | Validated; invalid input warned & dropped. |
| `MEMORY_BASE_DIR` | `memory.paths.base_dir` | |
| `MEMORY_PROFILE` | `memory.profile` | `basic` / `advanced` / `expert` |
| `MEMORY_SESSION_DIR` | `memory.session.base_dir` | |
| `MEMORY_SESSION_END` | `memory.session.end_action` | |
| `MEMORY_MOUNT_STRATEGY` | `memory.mount.strategy` | |
| `MEMORY_INDEX_ENABLED` | `memory.index.enabled` | systemd-style truthy/falsy |
| `MEMORY_AUDIT_JOURNALD` | `memory.audit.journald` | |
| `MEMORY_CGROUP_ENABLED` | `memory.cgroup.enabled` | |
| `MEMORY_CGROUP_MEMORY_MAX` | `memory.cgroup.memory_max` | `512M` / `2G` / bytes |
| `MEMORY_GIT_ENABLED` | `memory.git.enabled` | |
| `MEMORY_GIT_AUTO_COMMIT` | `memory.git.auto_commit` | |
| `MEMORY_MAX_READ_BYTES` | `memory.max_read_bytes` | |
| `MEMORY_MAX_WRITE_BYTES` | `memory.max_write_bytes` | |
| `MEMORY_MAX_APPEND_BYTES` | `memory.max_append_bytes` | |
| `MEMORY_SESSION_ID` | (runtime-only) | Pins the agent run to a specific session id under `MEMORY_SESSION_DIR`. Required for `mem_promote`; see § 7. |

### Profiles

Profiles are a UX hint (not a security boundary), enforced at both
`tools/list` and `tools/call`:

- **basic** — all 19 tools listed; weak models can still benefit from
  the structured Tier B API.
- **advanced** (default) — all 19 tools listed; strong models are
  expected to prefer Tier A file ops.
- **expert** — Tier B (`memory_search`, `memory_observe`,
  `memory_get_context`) is hidden from `tools/list` and rejected at
  `tools/call` with `METHOD_NOT_FOUND`. Frontier models that already
  know how to navigate a filesystem need only Tier A and Tier C.

---

## 5. Feature Reference

### Tier A — File operations (11 tools)

`mem_read` / `mem_write` / `mem_append` / `mem_edit` / `mem_list` /
`mem_grep` / `mem_diff` / `mem_mkdir` / `mem_remove` / `mem_promote` /
`mem_session_log`.

The agent thinks in mount-relative paths. Reserved prefixes (`.anolisa`,
`.git`, `.gitignore`) are refused at write time. `mem_edit` requires
exactly one match for `old_str` (zero or many → error) so it cannot
quietly clobber the wrong region. `mem_promote` moves a file from the
session's `scratch/` into the persistent store atomically.

### Tier B — Structured search (3 tools)

`memory_search` runs a BM25 query against the FTS5 index and returns
ranked snippets. `memory_observe` writes a small frontmatter +
content blob under `notes/observed/<ULID>.md` so the agent has a
zero-decision way to record a thought. `memory_get_context` assembles
a token-bounded markdown preview of the most recently modified files —
useful at the start of a turn to remind the agent what's in store.

### Tier C — Governance (5 tools)

`mem_snapshot` / `mem_snapshot_list` / `mem_snapshot_restore` give
mount-wide point-in-time backups (tar.gz with sidecar metadata).
`mem_log` and `mem_revert` operate on the optional git mirror — useful
for "I edited the wrong file three turns ago" recovery.

### Sandbox guarantees

- Path traversal (`..`, absolute paths, `\0`) → kernel-rejected by
  `openat2`.
- Symlink swap mid-call → kernel-rejected by `RESOLVE_NO_SYMLINKS`;
  recursive removal uses `fdopendir` + `fstatat(AT_SYMLINK_NOFOLLOW)`
  + `unlinkat` so swaps cannot race.
- Reserved-path overwrite (`.anolisa/audit.log`, `.gitignore`, ...) →
  rejected by `TargetIsReserved`.
- Oversize payloads → rejected against `max_*_bytes` caps.
- `mem_snapshot_restore`-induced symlink injection → tarball entry-type
  filter rejects `Symlink` / `Hardlink` / `Device` / `Fifo`.

---

## 6. Tool API Reference

All tools speak MCP `tools/call` with a JSON arguments object. Errors
come back as `CallToolResult { isError: true, content: [{type: "text",
text: "<reason>"}] }` so a client can branch on `isError`.

### Tier A

| Tool | Required | Optional | Returns |
|------|----------|----------|---------|
| `mem_read` | `path` | — | UTF-8 file content |
| `mem_write` | `path`, `content` | `overwrite` | `wrote N bytes to <path>` |
| `mem_append` | `path`, `content` | — | `appended N bytes to <path>` |
| `mem_edit` | `path`, `old_str`, `new_str` | — | `edited <path>` |
| `mem_list` | — | `dir`, `recursive`, `glob` | JSON array of `{name, type, size, mtime}` |
| `mem_grep` | `pattern` | `dir`, `type`, `max`, `case_insensitive` | JSON array of `{path, line, text}` |
| `mem_diff` | `path1`, `path2` | — | unified diff |
| `mem_mkdir` | `path` | — | `created <path>` |
| `mem_remove` | `path` | `recursive` | `removed <path>` |
| `mem_promote` | `session_path`, `store_path` | — | `promoted N bytes: <src> -> <dst>` |
| `mem_session_log` | — | — | session JSONL or `(session log is empty)` |

### Tier B

| Tool | Required | Optional | Returns |
|------|----------|----------|---------|
| `memory_search` | `query` | `top_k` (default 5) | JSON array of `{path, score, snippet}` |
| `memory_observe` | `content` | `hint` | `observed at notes/observed/<ulid>.md` |
| `memory_get_context` | — | `max_tokens` (default 2048) | markdown preview |

### Tier C

| Tool | Required | Optional | Returns |
|------|----------|----------|---------|
| `mem_snapshot` | — | `name` | JSON `{id, name, created_at, size, backend}` |
| `mem_snapshot_list` | — | — | JSON array, oldest → newest |
| `mem_snapshot_restore` | `id` | — | `restored <id>` |
| `mem_log` | — | `limit` (default 20), `path` | JSON array of `{hash, summary, author, time}` |
| `mem_revert` | `path` | — | `reverted <path> (commit <hash>)` |

### Error code semantics

| MCP error code | Meaning |
|----------------|---------|
| `-32601` METHOD_NOT_FOUND | tool hidden under current profile |
| `-32602` INVALID_PARAMS | missing / mistyped argument |
| `-32603` INTERNAL_ERROR | server-side failure |
| `isError: true` content | tool ran but returned a domain error (path not found, sandbox refusal, size cap exceeded, ...) |

---

## 7. SDK / Client Integration Guide

### Wiring up MCP-compatible clients

#### Claude Code (`.claude/settings.json`)

```json
{
  "mcpServers": {
    "agent-memory": {
      "command": "/usr/bin/agent-memory",
      "args": [],
      "env": {
        "USER_ID": "alice",
        "MEMORY_PROFILE": "advanced"
      }
    }
  }
}
```

#### Cursor / Continue / any MCP client over stdio

Point the client at the binary with the same `command` / `args` /
`env` shape. The descriptor at
`/usr/share/anolisa/mcp-servers/agent-memory.json` lists the 19 tool
names so a client that auto-discovers MCP servers picks them up.

### Programmatic clients

#### Python (using the official `mcp` SDK)

```python
import asyncio
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

async def main():
    server = StdioServerParameters(
        command="/usr/bin/agent-memory",
        args=[],
        env={"USER_ID": "alice"},
    )
    async with stdio_client(server) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools = await session.list_tools()
            print([t.name for t in tools.tools])

            result = await session.call_tool(
                "mem_write",
                {"path": "notes/from-python.md", "content": "hello"},
            )
            assert not result.isError
            print(result.content[0].text)

asyncio.run(main())
```

#### TypeScript (`@modelcontextprotocol/sdk`)

```typescript
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const transport = new StdioClientTransport({
  command: "/usr/bin/agent-memory",
  args: [],
  env: { USER_ID: "alice" },
});
const client = new Client({ name: "my-app", version: "1.0.0" }, {});
await client.connect(transport);

const result = await client.callTool({
  name: "mem_grep",
  arguments: { pattern: "TODO", recursive: true, max: 50 },
});
console.log(result.isError ? "failed" : result.content);
```

#### Rust (via `rmcp`)

```rust
use rmcp::transport::child_process::ChildProcessTransport;
use rmcp::ServiceExt;

let transport = ChildProcessTransport::new(
    tokio::process::Command::new("/usr/bin/agent-memory"),
).await?;
let client = ().serve(transport).await?;
let tools = client.list_tools(Default::default()).await?;
let resp = client.call_tool(rmcp::model::CallToolRequestParam {
    name: "mem_read".into(),
    arguments: Some(serde_json::json!({"path": "notes/x.md"})
        .as_object().unwrap().clone()),
}).await?;
```

### Promote-flow integration (multi-turn pattern)

For agents that need a "draft now, persist on commit" pattern:

1. Set `MEMORY_SESSION_ID=<sid>` and
   `MEMORY_SESSION_DIR=/run/anolisa/sessions` per agent run.
2. Agent writes drafts to the session scratch (the runtime is
   responsible for staging files into
   `/run/anolisa/sessions/<sid>/scratch/`).
3. When the agent decides "this is worth keeping", call `mem_promote`
   to atomically move the file into the persistent store.

### Observability hooks

- `audit.journald=true` — fan out every call to
  `journalctl --user-unit=anolisa-memory@<user>`.
- `mem_session_log` — read the per-session JSONL from inside the agent
  to self-reflect on what it has done this turn.
- `mem_log` (with git enabled) — surface change history to the agent;
  combine with `mem_revert` to give it a real "undo" button.

---

## 8. Testing & Verification Guide

### 8.1 Automated tests

```bash
cd src/agent-memory
cargo fmt --check
cargo clippy -- -D warnings
cargo test                                        # all suites
cargo test --test e2e_agent_test                  # 19-tool E2E
cargo test --test mcp_integration_test            # protocol level
cargo test --test linux_userns_test -- --ignored  # needs unprivileged userns
```

The CI job in `ci.yaml` runs `fmt --check`, `clippy -D warnings`, and
`cargo test` on Rust 1.89.

### 8.2 Interactive `mcp-harness`

`mcp-harness` is an example binary that drives the server via stdio
and gives you a REPL for manual tool calls.

```bash
cargo run --example mcp-harness -- /tmp/mem-test
```

| Command | Description |
|---------|-------------|
| `list` | List all visible tools |
| `call <tool> <json_args>` | Invoke a tool |
| `help` | Command reference |
| `quit` | Tear down server, exit |

Sample session:

```
mcp> call mem_mkdir {"path": "notes"}
Result: created notes
mcp> call mem_write {"path": "notes/day1.md", "content": "Hello world"}
Result: wrote 11 bytes to notes/day1.md
mcp> call mem_read {"path": "notes/day1.md"}
Result: Hello world
```

Pre-built scenarios (no manual asserts; you visually verify):

```bash
cargo run --example mcp-harness -- /tmp/mem-test --scenario full
cargo run --example mcp-harness -- /tmp/mem-test --scenario git --git
cargo run --example mcp-harness -- /tmp/mem-test --scenario promote
cargo run --example mcp-harness -- /tmp/mem-test --verbose   # log JSON-RPC
```

### 8.3 Raw JSON-RPC (protocol-level debugging)

Start the server and pipe JSON-RPC lines to its stdin:

```bash
mkdir -p /tmp/mem-test/__sessions__
MEMORY_BASE_DIR=/tmp/mem-test \
MEMORY_SESSION_DIR=/tmp/mem-test/__sessions__ \
MEMORY_MOUNT_STRATEGY=userland \
USER_ID=tester \
agent-memory
```

Handshake:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"manual","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
```

Tool call:

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mem_write","arguments":{"path":"test.md","content":"hello"}}}
```

Expected response shape:

```json
{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"wrote 5 bytes to test.md"}],"isError":false}}
```

### 8.4 Sandbox verification

Confirm the kernel sandbox refuses each escape vector:

```json
{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"mem_read","arguments":{"path":"../../etc/passwd"}}}
```
→ `isError: true`, message `path outside mount root`.

```json
{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"mem_write","arguments":{"path":".anolisa/audit.log","content":"x"}}}
```
→ `isError: true`, message `target is reserved`.

```json
{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"mem_read","arguments":{"path":"a/b/symlink-to-etc-passwd"}}}
```
→ `isError: true`, message `path outside mount root` (kernel ELOOP).

### 8.5 Per-tool verification procedures

Each procedure assumes either the harness REPL (`call <tool> <json>`)
or raw JSON-RPC. Run inside `mcp-harness` for the shortest loop.

- **mem_mkdir** — `call mem_mkdir {"path":"d"}` → response contains
  `created`. Verify with `call mem_list {"recursive": true}`.
- **mem_write / mem_read** — write `Hello world\n`, read it back, byte
  match. Re-write with `overwrite=false` should error.
- **mem_append** — append `+more`, re-read, content equals
  `original+more`.
- **mem_edit** — write `foo bar baz`, edit `bar` → `qux`, read back
  `foo qux baz`. Repeat with `bar` (now absent) → error
  `match count 0`.
- **mem_list** — create nested dirs and files; recursive list shows all
  paths plus `README.md` from init.
- **mem_grep** — write two files containing distinct keywords; grep
  for one keyword surfaces only the matching file with `path / line /
  text`.
- **mem_diff** — diff two files, output starts with `--- ` / `+++ `
  unified-diff headers.
- **mem_remove** — remove a file, subsequent read errors `not found`.
- **mem_promote** — pre-create `MEMORY_SESSION_DIR/<sid>/scratch/x.md`,
  set env, call promote, read the destination.
- **mem_session_log** — call any 3 tools, then `mem_session_log` returns
  3 JSONL lines.
- **memory_observe** — observe twice; `mem_list notes/observed`
  recursively shows two ULID-named files.
- **memory_search** — observe with keyword `kappa`, wait ~500 ms,
  search for `kappa`, the observed file is in the result.
- **memory_get_context** — write 5 files with distinct first lines,
  `memory_get_context {max_tokens: 200}` previews them.
- **mem_snapshot / list** — snapshot, list, expect entry; size > 0;
  `id` starts with `snap_`.
- **mem_snapshot_restore** — write v1, snapshot, write v2, restore
  snapshot, read returns v1; `.anolisa/trash/<ts>-<id>/` contains v2.
- **mem_log** — enable git, write three versions of the same file,
  `mem_log {path: "..."}` returns ≥3 commits.
- **mem_revert** — enable git, write v3, revert, read returns the last
  committed (v2) content.

### 8.6 Smoke test (single command)

The Makefile ships a self-contained smoke test that drives 5 tools
through the server and verifies the responses:

```bash
cd src/agent-memory
make smoke
```

A green `==> Smoke test PASSED` is the minimum signal a deployment is
working end-to-end.

---

## 9. Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `unshare(NEWUSER\|NEWNS): EPERM` at startup | unprivileged user namespaces disabled | `sysctl kernel.unprivileged_userns_clone=1`, or set `MEMORY_MOUNT_STRATEGY=userland`. |
| `tmpfs /mnt: EBUSY` | something else owns `/mnt` in the new namespace | The retry path treats EBUSY as success; if it persists, restart the process. |
| `cargo build` fails on macOS / Windows with `libsystemd`/`nix` errors | host is not Linux | Use `make remote-build` / `remote-test`. |
| `tools/call memory_search` → `METHOD_NOT_FOUND` | `MEMORY_PROFILE=expert` hides Tier B | Switch to `advanced` or call file-tool equivalents. |
| Config typo silently ignored | the binary used to default-fill misspelt fields | This is now a hard error: read the load-time stderr message and fix the key. |
| `mem_log` returns `[]` even after writes | git versioning disabled | `MEMORY_GIT_ENABLED=true MEMORY_GIT_AUTO_COMMIT=true`. |
| Index search returns nothing for fresh content | inotify event still in the 200 ms debounce window | Retry; or call `mem_grep` (regex over filesystem, no index). |
| `mem_promote` errors `session not found` | `MEMORY_SESSION_ID` / `MEMORY_SESSION_DIR` not set or scratch missing | See § 7 promote-flow integration. |

For deeper diagnosis, run with `RUST_LOG=agent_memory=debug` and
inspect both the server stderr and `<mount>/.anolisa/audit.log`.

---

## License

Apache-2.0. See `LICENSE` shipped with the package.

## Reporting issues

[`github.com/alibaba/anolisa/issues`](https://github.com/alibaba/anolisa/issues),
component `memory`.
