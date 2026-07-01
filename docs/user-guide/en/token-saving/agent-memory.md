# Agent Memory

Agent Memory provides MCP-based persistent file memory for AI Agents. It enables Agents to retain context across sessions by storing structured memories as files, accessible via the Model Context Protocol (MCP).

---

## Overview

AI Agents typically lose all context between sessions. Agent Memory solves this by providing:

- **Persistent Storage** — memories survive across Agent restarts and sessions
- **File-based Architecture** — memories stored as structured files for transparency and portability
- **MCP Interface** — standard Model Context Protocol server for seamless Agent integration
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

### Option 2: Source build (developers)

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

## Configuration

Configuration file: `~/.config/agent-memory/config.toml`

```toml
[storage]
# Directory for memory files
path = "~/.local/share/agent-memory"

[server]
# MCP server transport
transport = "stdio"
```

---

## FAQ

**Q: Where are memories stored?**
A: By default in `~/.local/share/agent-memory/` as structured files.

**Q: Can Agent Memory work in sandboxed environments?**
A: Yes. Agent Memory is designed to operate within restricted/sandboxed execution contexts.

**Q: How does this differ from Tokenless?**
A: Tokenless compresses in-context information to save Tokens. Agent Memory offloads knowledge to persistent storage so it doesn't need to be in-context at all.
