# Agent Memory (agent-memory)

agent-memory is ANOLISA's file-form memory MCP server, providing AI agents with a persistent, searchable, sandboxed memory space. Agents read and write memory like a filesystem; the system injects relevant context into subsequent turns via BM25/vector hybrid retrieval and automatic capture/recall, reducing repeated communication and improving task continuity.

- **File-form memory**: read/write memory with filesystem semantics via MCP tools; namespace isolation and path sandboxing.
- **Hybrid semantic search**: BM25 + dense vector + RRF fusion with automatic fallback.
- **Auto capture & recall**: automatically extracts observations at conversation end (deduped) and injects relevant memory when building the next prompt.
- **Safe injection**: prompt-injection detection and escaping for memory content injected into LLM prompts.
- **Versioning & snapshots**: optional auto git commit + tar.gz snapshots for file-level and mount-level rollback.

---

## Installation

### Via anolisa CLI (recommended)

```bash
anolisa install agent-memory
```

Produces: `agent-memory` binary, default config, MCP service descriptor, systemd user template, tmpfiles rule, OpenClaw adapter bundle.

### RPM package (AnolisOS / RHEL)

```bash
sudo yum install agent-memory
```

RPM installs to system-level FHS paths:

| Purpose | Path |
|------|------|
| Service binary | `/usr/bin/agent-memory` |
| Default config | `/usr/share/anolisa/agent-memory/default.toml` |
| MCP service descriptor (auto-discovery) | `/usr/share/anolisa/mcp-servers/agent-memory.json` |
| systemd user template | `/usr/lib/systemd/user/anolisa-memory@.service` |
| tmpfiles rule (creates `/run/anolisa/{,sessions}`) | `/usr/lib/tmpfiles.d/anolisa-memory.conf` |
| OpenClaw adapter bundle | `/usr/share/anolisa/adapters/agent-memory/` |
| Docs | `/usr/share/doc/agent-memory/` |

### Source build (developers)

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa/src/agent-memory

make build         # cargo build --release --locked
sudo make install  # install to /usr/local
```

Build deps: Rust ≥ 1.85 (edition 2024; CI pins 1.89 to share the monorepo toolchain), cmake (libgit2 vendored), systemd-devel (journald audit fan-out).

### Cross-platform development

Runtime is Linux-only (depends on user_namespace, mount(2), cgroup v2, inotify, journald). On macOS / Windows use the remote flow:

```bash
make remote-build   # push branch and ssh to a Linux host for cargo build
make remote-test    # same + tests + clippy
```

---

## Integration

### Claude Code / Cursor / Continue / any stdio MCP client

Add to your MCP config:

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

`/usr/share/anolisa/mcp-servers/agent-memory.json` lists all 37 tool names for auto-discovering clients.

### OpenClaw

The bundled plugin forwards 4 memory-contract tools (`memory_search`, `memory_get`, `memory_observe`, `memory_get_context`) to agent-memory:

```bash
bash /usr/share/anolisa/adapters/agent-memory/openclaw/scripts/install.sh
openclaw gateway restart
```

Or via anolisa adapter management:

```bash
anolisa adapter enable agent-memory openclaw
anolisa adapter status agent-memory
```

**Prerequisite**: `openclaw` CLI on `$PATH`. The script logs clearly and exits 0 if missing — rerun after installing OpenClaw. `yum remove agent-memory` triggers `%preun` to call the uninstall script, leaving no orphaned config.

Plugin contract ↔ agent-memory MCP tool mapping:

| OpenClaw contract | agent-memory MCP tool |
|---|---|
| `memory_search` | `memory_search` (BM25 default; `mode=vector\|hybrid` with embedding) |
| `memory_get` | `mem_read` |
| `memory_observe` | `memory_observe` |
| `memory_get_context` | `memory_get_context` |

Plugin config (via OpenClaw UI or `openclaw.json` `plugins.entries["memory-anolisa"].config`):

| Key | Default | Purpose |
|---|---|---|
| `binaryPath` | auto-discovery: `$PATH` → `/usr/bin/agent-memory` → `/usr/local/bin/agent-memory` → `~/.local/bin/agent-memory` | absolute binary path |
| `userId` | env `USER_ID` → OS `uid` → env `$USER` | namespace `user_id`; same validation as Rust side |
| `profile` | `advanced` | profile gate, passed as `MEMORY_PROFILE` env |
| `maxReadBytes` | `1048576` (1 MiB) | `mem_read` cap, passed as `MEMORY_MAX_READ_BYTES` |
| `maxWriteBytes` | `16777216` (16 MiB) | `mem_write` cap, passed as `MEMORY_MAX_WRITE_BYTES` |
| `sessionId` | env `MEMORY_SESSION_ID` → new `ses_<random>` | namespace session; must be fixed |
| `sessionDir` | env `MEMORY_SESSION_DIR` → `/run/anolisa/sessions` | session scratch + log root |

The plugin passes a minimal env allowlist to the subprocess (`PATH`, `HOME`, `USER`, `USER_ID`, `LANG`/`LC_ALL`/`LC_CTYPE`, `TZ`, `TMPDIR`, `XDG_RUNTIME_DIR`, and all `MEMORY_`/`RUST_`-prefixed vars); other env does not leak. `USER_ID` matches exactly — `USER_IDX` is not allowed.

---

## MCP tool set (37 tools)

All tools are invoked via MCP `tools/call` with JSON object arguments. Errors return `CallToolResult { isError: true }` so clients can distinguish business errors from "successful but content contains 'failed'". Profile is enforced at both `tools/list` and `tools/call`.

### Tier A — file operations (11)

| Tool | Required | Optional | Returns |
|------|------|------|------|
| `mem_read` | `path` | — | UTF-8 file content |
| `mem_write` | `path`, `content` | `overwrite` | `wrote N bytes to <path>` |
| `mem_append` | `path`, `content` | — | `appended N bytes to <path>` |
| `mem_edit` | `path`, `old_str`, `new_str` | — | `edited <path>` (`old_str` must match exactly once) |
| `mem_list` | — | `dir`, `recursive`, `glob` | `{name, type, size, mtime}` array |
| `mem_grep` | `pattern` | `dir`, `type`, `max`, `case_insensitive` | `{path, line, text}` array |
| `mem_diff` | `path1`, `path2` | — | unified diff |
| `mem_mkdir` | `path` | — | `created <path>` |
| `mem_remove` | `path` | `recursive` | `removed <path>` |
| `mem_promote` | `session_path`, `store_path` | — | atomically move session scratch file into the persistent store |
| `mem_session_log` | — | — | current session JSONL |

### Tier B — structured retrieval (6)

| Tool | Required | Optional | Returns |
|------|------|------|------|
| `memory_search` | `query` | `top_k` (default 5), `mode` (bm25/vector/hybrid), `category` | `{path, score, snippet, suspicious}` array |
| `memory_observe` | `content` | `hint`, `type` | `observed at notes/observed/<ulid>.md` |
| `memory_get_context` | — | `max_tokens` (default 2048) | markdown preview of recently modified files; each entry has `suspicious` |
| `memory_sessions` | — | `limit` (default 10) | historical session list |
| `memory_timeline` | — | — | cross-session timeline |
| `mem_index_refresh` | — | — | force-rebuild the FTS5 index |

### Tier C — governance & versioning (7)

| Tool | Required | Optional | Returns |
|------|------|------|------|
| `mem_snapshot` | — | `name` | `{id, name, created_at, size, backend}` |
| `mem_snapshot_list` | — | — | array sorted by `created_at` |
| `mem_snapshot_restore` | `id` | — | `restored <id>` |
| `mem_log` | — | `limit` (default 20), `path` | `{hash, summary, author, time}` array (requires git) |
| `mem_revert` | `path` | — | `reverted <path> (commit <hash>)` (requires git) |
| `mem_consolidate` | — | — | `consolidation complete: N facts written` |
| `mem_compact` | — | — | `compacted N files to cold storage` |

### Sovereignty & import/export (13)

| Tool | Description |
|------|------|
| `memory_about` | memory store metadata |
| `memory_auto_created` | query auto-extracted facts |
| `memory_consent` | grant/revoke memory operations |
| `memory_forget` | delete specific memory entries |
| `mem_export` | export the memory store to an archive |
| `mem_import` | import memory from an archive |
| `memory_task_save` | save task context |
| `memory_task_list` | list saved tasks |
| `memory_task_resume` | resume task context |
| `memory_task_close` | close a task |
| `memory_summary` | memory store statistics overview |
| `memory_session_context` | session-start context injection |
| `mem_dream` | user profile synthesis |

### Error code semantics

| MCP code | Meaning |
|------------|------|
| `-32601` METHOD_NOT_FOUND | tool hidden by current profile |
| `-32602` INVALID_PARAMS | missing or wrong-type param |
| `-32603` INTERNAL_ERROR | server fault |
| `isError: true` | tool ran but returned a business error (path missing, sandbox rejection, size limit, etc.) |

---

## Core features

### File-form memory

Agents organize memory by path, matching the human filesystem model:

```
notes/day1.md
decisions/2026-05/db-pick.md
context/project-overview.md
```

Namespace layout:

```
~/.anolisa/memory/user-<uid>/        # mount root
├── README.md                        # auto-generated overview
├── notes/                           # free-form notes
├── decisions/                       # user-defined subdirs
└── .anolisa/                        # OS-managed, not writable by agents
    ├── manifest.toml                # namespace metadata
    ├── audit.log                    # JSONL tool-call audit
    ├── index.db                     # FTS5 SQLite
    ├── snapshots/                   # tar.gz archives + sidecar
    ├── trash/                       # entries retained on restore
    └── git/                         # bare git mirror (when git enabled)
```

Session dir (tmpfs, 0700):

```
/run/anolisa/sessions/<sid>/
├── meta.toml
├── log.jsonl
└── scratch/                         # session-only drafts; promoted via mem_promote
```

### Sandbox protection

Every file open is anchored at the mount root via kernel `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)`:

- Rejects `..` traversal
- Rejects symlinks (including mid-call replacement; recursive deletes use `fdopendir` + `fstatat(AT_SYMLINK_NOFOLLOW)` + `unlinkat` so swaps can't race)
- Rejects access to metadata dirs (`.anolisa`, `.git`, `.gitignore` via `TargetIsReserved`)
- `mem_snapshot_restore` filters tar entry types — rejects `Symlink`/`Hardlink`/`Device`/`Fifo`
- Oversized payloads rejected per `max_*_bytes`

**Mount strategies**:

| Strategy | When | Behavior |
|------|------|------|
| `userland` (default) | any environment | mount is just a directory; sandbox enforced by `openat2` |
| `userns` | Linux ≥ 4.6 with unprivileged user namespace | `unshare` into a new user+mount namespace, mount a private tmpfs, then bind-mount the backing dir; host-side processes can't see `/mnt/memory/<ns>/` |
| `auto` | runtime probe | try `userns`; fall back to `userland` on any error |

### Version control

Optional auto git commit (libgit2 vendored):

```bash
MEMORY_GIT_ENABLED=true MEMORY_GIT_AUTO_COMMIT=true agent-memory
```

With git on, `mem_log` exposes change history and `mem_revert` gives the agent a real "undo" button. `mem_snapshot*` provides mount-wide tar.gz point-in-time backups independent of git.

### Full-text search

SQLite FTS5 BM25 index, sub-millisecond queries. A background tokio task watches the mount via `inotify`; events are debounced 200 ms and applied in a single transaction. Tokenizer is `trigram` (CJK-friendly). `IN_Q_OVERFLOW` triggers a full rescan — events are never silently dropped.

### Hybrid vector search

BM25 + dense vector hybrid retrieval, fused via RRF (Reciprocal Rank Fusion, k=60). Vectors come from a pluggable Embedding Provider:

| Provider | Configuration | Notes |
|----------|---------|------|
| OpenAI | `MEMORY_EMBEDDING_BACKEND=openai` + `OPENAI_API_KEY` | calls OpenAI Embeddings API |
| Ollama | `MEMORY_EMBEDDING_BACKEND=ollama` + `OLLAMA_BASE_URL` | local Ollama instance |

`memory_search` supports `mode`: `bm25` (default) / `vector` (cosine similarity) / `hybrid` (RRF fusion). Without embedding config, `vector`/`hybrid` auto-degrade to BM25 — no error.

### Auto consolidation

On shutdown, automatically extracts atomic facts from the session audit log (`mem_consolidate`) using 6 heuristic rules (zero LLM calls) — identifies high-frequency paths, search patterns, etc., and persists them as structured memory. Also manually triggerable via the `mem_consolidate` tool. Includes episodic memory extraction and conflict detection (BM25 threshold).

### Audit & observability

Every successful tool call appends a JSONL line to `<mount>/.anolisa/audit.log`; with sessions enabled, also to `/run/anolisa/sessions/<sid>/log.jsonl`. `audit.journald=true` fans out to systemd-journald with structured fields (`MESSAGE_ID`, `AGENT_MEMORY_TOOL`, etc.) for `journalctl --user-unit=anolisa-memory@<user>` filtering.

---

## Configuration

### Config file

Default location: `~/.anolisa/memory.toml`. All structs enable `serde(deny_unknown_fields)` — typos hard-fail at load. Minimal config:

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
time_decay_lambda = 0.01
time_decay_alpha = 0.3
cold_after_days = 30
exclude_cold_on_search = true

[memory.audit]
journald = false

[memory.cgroup]
enabled = false
memory_max = "512M"

[memory.git]
enabled = false
auto_commit = true

[memory.consolidation]
enabled = true
max_facts = 20
min_tool_calls = 3
episodic_enabled = true
min_episode_steps = 3
max_episodes_per_session = 10
conflict_detection = true
conflict_bm25_threshold = -2.0
```

### Environment variables

Every config key has a matching `MEMORY_*` env var. Priority: **env > config.toml > default**.

| Variable | Description | Default |
|----------|------|------|
| `USER_ID` | user identity (validated; invalid values warn-and-ignore) | — |
| `MEMORY_PROFILE` | profile (basic/advanced/expert) | advanced |
| `MEMORY_BASE_DIR` | memory store root | `~/.anolisa/memory` |
| `MEMORY_SESSION_DIR` | session root | `/run/anolisa/sessions` |
| `MEMORY_SESSION_ID` | fixed session id (required for `mem_promote`) | new `ses_<random>` |
| `MEMORY_SESSION_END` | session end action (discard/keep) | discard |
| `MEMORY_MOUNT_STRATEGY` | mount strategy (auto/userland/userns) | auto |
| `MEMORY_MAX_READ_BYTES` | per-read cap | 1 MiB |
| `MEMORY_MAX_WRITE_BYTES` | per-write cap | 16 MiB |
| `MEMORY_MAX_APPEND_BYTES` | per-append cap | 4 MiB |
| `MEMORY_INDEX_ENABLED` | enable FTS5 index | true |
| `MEMORY_INDEX_TIME_DECAY_LAMBDA` | time decay (≥0) | 0.01 |
| `MEMORY_INDEX_TIME_DECAY_ALPHA` | time weight ratio (0–1) | 0.3 |
| `MEMORY_INDEX_COLD_AFTER_DAYS` | cold archive days | 30 |
| `MEMORY_INDEX_EXCLUDE_COLD` | exclude cold from search | true |
| `MEMORY_AUDIT_JOURNALD` | fan out to journald | false |
| `MEMORY_CGROUP_ENABLED` | enable cgroup limits | false |
| `MEMORY_CGROUP_MEMORY_MAX` | cgroup memory cap | 512M |
| `MEMORY_GIT_ENABLED` | enable git versioning | false |
| `MEMORY_GIT_AUTO_COMMIT` | auto commit | true |
| `MEMORY_EMBEDDING_BACKEND` | embedding backend (none/openai/ollama) | none |
| `MEMORY_OPENAI_API_KEY` | OpenAI API key (falls back to `OPENAI_API_KEY`) | — |
| `MEMORY_OPENAI_MODEL` | OpenAI embedding model | text-embedding-3-small |
| `MEMORY_OLLAMA_MODEL` | Ollama embedding model | nomic-embed-text |
| `MEMORY_OLLAMA_BASE_URL` | Ollama base URL | http://localhost:11434 |
| `MEMORY_CONSOLIDATION_ENABLED` | enable auto consolidation | true |
| `MEMORY_CONSOLIDATION_MAX_FACTS` | max facts per run | 20 |
| `MEMORY_CONSOLIDATION_MIN_CALLS` | min tool-call threshold | 3 |
| `MEMORY_EPISODIC_ENABLED` | episodic extraction | true |
| `MEMORY_MIN_EPISODE_STEPS` | min episode steps | 3 |
| `MEMORY_MAX_EPISODES` | max episodes per session | 10 |
| `MEMORY_CONFLICT_DETECTION` | conflict detection | true |
| `MEMORY_CONFLICT_THRESHOLD` | BM25 conflict threshold | -2.0 |

Data storage: `~/.anolisa/memory/<namespace>/`.

### Profiles

Profiles are UX hints, not security boundaries, but enforced at both `tools/list` and `tools/call`:

- **basic** — all 37 tools shown; weaker models can use the Tier B structured API.
- **advanced** (default) — all 37 tools shown; stronger models should prefer Tier A file ops.
- **expert** — hides Tier B (`memory_search`, `memory_observe`, `memory_get_context`, `mem_consolidate`, `memory_forget`, `memory_consent`); `tools/call` returns `METHOD_NOT_FOUND`. For proficient models that only need Tier A and Tier C.

### Embedding config

```toml
[memory.embedding]
backend = "openai"                # or "ollama"
api_key = ""                      # empty: auto-read OPENAI_API_KEY
model = "text-embedding-3-small"
# Ollama: backend = "ollama", model = "nomic-embed-text", base_url = "http://localhost:11434"
```

---

## Use cases

- Cross-session persistence of notes and decisions (Claude Code, Cursor, Continue, custom rmcp clients).
- Multi-agent systems where Agent A writes and Agent B reads shared notes.
- Operation audit and state recovery (`mem_log`, JSONL audit, journald, `mem_revert`, `mem_snapshot_restore`).
- Multi-turn "draft first, persist when decided" pattern (`mem_promote` atomically moves files from session scratch into the persistent store).

---

## SDK / client integration

### Python (official `mcp` SDK)

```python
import asyncio
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

async def main():
    server = StdioServerParameters(
        command="/usr/bin/agent-memory", args=[],
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

asyncio.run(main())
```

### TypeScript (`@modelcontextprotocol/sdk`)

```typescript
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const transport = new StdioClientTransport({
  command: "/usr/bin/agent-memory", args: [],
  env: { USER_ID: "alice" },
});
const client = new Client({ name: "my-app", version: "1.0.0" }, {});
await client.connect(transport);
const result = await client.callTool({
  name: "mem_grep",
  arguments: { pattern: "TODO", recursive: true, max: 50 },
});
```

### Rust (`rmcp`)

```rust
use rmcp::transport::child_process::ChildProcessTransport;
use rmcp::ServiceExt;

let transport = ChildProcessTransport::new(
    tokio::process::Command::new("/usr/bin/agent-memory"),
).await?;
let client = ().serve(transport).await?;
let tools = client.list_tools(Default::default()).await?;
```

### Promote workflow (multi-turn)

1. Set `MEMORY_SESSION_ID=<sid>` and `MEMORY_SESSION_DIR=/run/anolisa/sessions` for each agent run.
2. Agent writes drafts to `/run/anolisa/sessions/<sid>/scratch/`.
3. When a draft is worth keeping, the agent calls `mem_promote` to atomically move it into the persistent store.

---

## Testing & verification

### Automated tests

```bash
cd src/agent-memory
cargo fmt --check
cargo clippy -- -D warnings
cargo test                              # full suite
cargo test --test e2e_agent_test        # tool E2E
cargo test --test mcp_integration_test  # protocol layer
cargo test --test linux_userns_test -- --ignored  # needs unprivileged userns
make smoke                              # one-shot end-to-end smoke
```

CI runs `fmt --check` + `clippy -D warnings` + `cargo test` on Rust 1.89.

### Interactive `mcp-harness`

```bash
cargo run --example mcp-harness -- /tmp/mem-test
```

| Command | Description |
|------|------|
| `list` | list visible tools |
| `call <tool> <json_args>` | invoke a tool |
| `help` | help |
| `quit` | quit |

Scenarios: `--scenario full` / `git --git` / `promote` / `--verbose` (prints JSON-RPC).

### Raw JSON-RPC (protocol-level debugging)

```bash
mkdir -p /tmp/mem-test/__sessions__
MEMORY_BASE_DIR=/tmp/mem-test \
MEMORY_SESSION_DIR=/tmp/mem-test/__sessions__ \
MEMORY_MOUNT_STRATEGY=userland \
USER_ID=tester \
agent-memory
```

Handshake + tool call:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"manual","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mem_write","arguments":{"path":"test.md","content":"hello"}}}
```

### Sandbox escape verification

```json
{"name":"mem_read","arguments":{"path":"../../etc/passwd"}}
```
→ `isError: true`, message `path outside mount root`.

```json
{"name":"mem_write","arguments":{"path":".anolisa/audit.log","content":"x"}}
```
→ `isError: true`, message `target is reserved`.

---

## Troubleshooting

### Diagnostic tools

```bash
# Component-level diagnosis + auto-fix
anolisa doctor agent-memory --fix

# Adapter status
anolisa adapter status agent-memory

# Debug startup
RUST_LOG=agent_memory=debug agent-memory
```

### Common issues

| Symptom | Likely cause | Fix |
|------|----------|------|
| startup `unshare(NEWUSER\|NEWNS): EPERM` | unprivileged user namespace disabled | `sysctl kernel.unprivileged_userns_clone=1`, or `MEMORY_MOUNT_STRATEGY=userland` |
| `tmpfs /mnt: EBUSY` | `/mnt` occupied in new namespace | restart the process |
| macOS / Windows `cargo build` fails on `libsystemd`/`nix` | non-Linux host | `make remote-build` / `remote-test` |
| `tools/call memory_search` returns `METHOD_NOT_FOUND` | `MEMORY_PROFILE=expert` hides Tier B | switch to `advanced`, or use Tier A directly |
| config typos silently ignored | — | now hard-fail; check startup stderr |
| `mem_log` returns `[]` despite writes | git versioning not enabled | `MEMORY_GIT_ENABLED=true MEMORY_GIT_AUTO_COMMIT=true` |
| search misses just-written content | inside the 200 ms debounce window | retry, or use `mem_grep` (regex on the filesystem, no index) |
| `mem_promote` reports `session not found` | `MEMORY_SESSION_ID`/`MEMORY_SESSION_DIR` unset or scratch missing | see Promote workflow |
| OpenClaw plugin not loaded | `openclaw` CLI not on PATH | rerun `install.sh` after installing OpenClaw |
| state out of sync after manual dnf | — | `anolisa repair agent-memory` / `anolisa forget` / `anolisa adopt` |

For deeper investigation: start with `RUST_LOG=agent_memory=debug` and inspect both stderr and `<mount>/.anolisa/audit.log`.

---

**License**: Apache-2.0
**Version**: 0.1.0
**Document version**: 2.0 (aligned with ANOLISA-design user-guide structure)
