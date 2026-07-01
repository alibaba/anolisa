# Agent Memory

Agent Memory provides MCP-based persistent file memory for AI Agents. It enables Agents to retain context across sessions by storing structured memories as files, accessible via the Model Context Protocol (MCP).

---

## Overview

AI Agents typically lose all context between sessions. Agent Memory solves this by providing:

- **Persistent Storage** — memories survive across Agent restarts and sessions
- **File-based Architecture** — memories stored as structured files for transparency and portability
- **MCP Interface** — standard Model Context Protocol server with 30+ tools for seamless Agent integration
- **Sandboxed Execution** — operates safely within restricted environments

---

## Prerequisites

- Linux (x86_64 or aarch64)
- An MCP-compatible Agent runtime

---

## Installation

### Option 1: anolisa CLI (recommended)

```bash
anolisa install agent-memory
```

### Option 2: YUM (Alinux, requires ANOLISA YUM repo)

```bash
sudo yum install agent-memory
```

### Option 3: Source build (developers)

```bash
cd src/agent-memory && make build
```

---

## Quick Start

```bash
# 1. Install Agent Memory
anolisa install agent-memory

# 2. Start the MCP server
agent-memory serve

# 3. Configure your Agent runtime to connect to the MCP server
# (see Integration section below)
```

---

## Integration

Agent Memory runs as an MCP server. Configure your Agent runtime to connect:

```json
{
  "mcpServers": {
    "agent-memory": {
      "command": "agent-memory",
      "args": ["serve"]
    }
  }
}
```

The Agent can then use MCP tools to read/write memories during conversation.

---

## MCP Tools

Agent Memory exposes 30+ MCP tools. Key categories:

### File Operations

- `mem_read` / `mem_write` / `mem_append` / `mem_edit` — read, write, append, and edit memory files
- `mem_list` / `mem_grep` / `mem_diff` — list, search, and diff memory content
- `mem_mkdir` / `mem_remove` — manage memory directories and files
- `mem_promote` — promote a memory entry

### Session & Context

- `mem_session_log` — log session activity
- `memory_search` / `memory_observe` / `memory_get_context` — semantic search and context retrieval
- `memory_sessions` / `memory_timeline` / `memory_summary` — session history and summaries

### Maintenance

- `mem_dream` / `mem_consolidate` / `mem_compact` — background consolidation and compaction
- `mem_index_refresh` — refresh the memory index
- `mem_snapshot` / `mem_snapshot_list` / `mem_snapshot_restore` — snapshot management
- `mem_log` / `mem_revert` — history log and revert

### Task Management

- `memory_task_save` / `memory_task_resume` / `memory_task_list` / `memory_task_close` — save and resume multi-step tasks

### Import/Export & Meta

- `mem_export` / `mem_import` — bulk export and import
- `memory_about` / `memory_forget` / `memory_auto_created` / `memory_consent` — metadata and controls

---

## Configuration

Configuration file: `~/.anolisa/memory.toml`

This file is **optional** and is not auto-generated. When absent, Agent Memory uses built-in defaults. Create it only if you need to override default behavior.

```toml
# Example: override defaults
[storage]
path = "~/.anolisa/memory/"

[server]
transport = "stdio"
```

### Data Directory

Memory files are stored in `~/.anolisa/memory/` by default.

---

## FAQ

**Q: Where are memories stored?**
A: By default in `~/.anolisa/memory/` as structured files.

**Q: Is a config file required?**
A: No. Agent Memory works with built-in defaults. The optional config at `~/.anolisa/memory.toml` is only needed to override specific settings.

**Q: Can Agent Memory work in sandboxed environments?**
A: Yes. Agent Memory is designed to operate within restricted/sandboxed execution contexts.

**Q: How does this differ from Tokenless?**
A: Tokenless compresses in-context information to save Tokens. Agent Memory offloads knowledge to persistent storage so it doesn't need to be in-context at all.
