# Agent Memory

[中文版](README_zh.md)

CMA-style persistent filesystem memory for AI agents, served over MCP. Provides sandboxed file tools, hybrid BM25 + vector search, auto capture/recall, git versioning, and tar.gz snapshots. Agent Memory is a memory component of [ANOLISA](../../README.md). Linux only.

## Features

- **File-form memory** — read/write with filesystem semantics via 37 MCP tools; namespace isolation and path sandboxing (openat2 RESOLVE_BENEATH)
- **Hybrid semantic search** — BM25 keyword + dense vector embeddings with reciprocal rank fusion (RRF); time-decay ranking
- **Auto capture & recall** — observes at conversation end, injects relevant context before the next prompt
- **Memory consolidation** — automatic extraction of atomic facts from session audit logs
- **Versioning & snapshots** — optional git auto-commit + tar.gz snapshots for file-level and mount-level rollback
- **Safety** — prompt-injection detection and secret/PII redaction for injected content
- **Cross-session tasks** — save/resume/close tasks across sessions with full context

## Quick Start

### Install

```bash
# Recommended
anolisa install agent-memory

# Or via RPM (Alinux)
sudo yum install agent-memory
```

### Integration (MCP client)

Add to your MCP config (Claude Code, Cursor, etc.):

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

### Core Operations

```bash
# Initialize namespace
agent-memory init

# Print resolved config
agent-memory info
```

Once running as MCP server, agents interact via tools:

| Operation | MCP Tool |
|-----------|----------|
| Write memory | `mem_write(path, content)` |
| Read memory | `mem_read(path)` |
| Search | `memory_search(query, mode="hybrid")` |
| Observe | `memory_observe(content, type)` |
| Get context | `memory_get_context(max_tokens)` |
| Snapshot | `mem_snapshot(name)` |

## Architecture

Single-process Tokio async runtime exposing 37 MCP tools over stdio JSON-RPC 2.0:

- **Tier A** (11 tools): file operations — read, write, append, edit, list, grep, diff, mkdir, remove, promote, session_log
- **Tier B** (6 tools): structured retrieval — search, observe, get_context, sessions, timeline, index_refresh
- **Tier C** (7 tools): governance — snapshot, restore, git log/revert, consolidate, compact
- **Sovereignty** (13 tools): about, forget, consent, export/import, tasks, dream

Profile gating (basic/advanced/expert) controls tool visibility per deployment.

## Requirements

- Linux (x86_64 / aarch64)
- Rust ≥ 1.85 (for source build)
- Optional: embedding provider (OpenAI or Ollama) for vector search

## License

Apache License 2.0 — see [LICENSE](LICENSE).
