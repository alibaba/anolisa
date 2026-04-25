# Token-Less OpenClaw Plugin

Unified OpenClaw plugin that combines **RTK command rewriting** and **tokenless schema/response compression** for 60–90% LLM token savings.

## Features

| Feature | Binary | Hook | Description |
|---------|--------|------|-------------|
| Command rewriting | `rtk` | `before_tool_call` | Rewrites `exec` tool commands to RTK equivalents |
| Schema compression | `tokenless` | `before_tool_register` | Compresses tool schemas before they enter the context window |
| Response compression | `tokenless` | `before_tool_response` | Compresses tool responses before they enter the context window |

Each feature degrades gracefully — if the corresponding binary is not installed, that feature is silently disabled.

## Prerequisites

- **rtk** `>= 0.28.0` — for command rewriting ([install guide](https://github.com/rtk-ai/rtk))
- **tokenless** `>= 0.1.0` — for schema/response compression

Both binaries must be available in `$PATH`.

## Installation

### Manual

Copy the `openclaw/` directory into your OpenClaw plugins folder:

```bash
cp -r openclaw/ ~/.openclaw/plugins/tokenless-openclaw/
```

### Via OpenClaw CLI

```bash
openclaw plugins install @tokenless/openclaw-plugin
```

## Configuration

All options are set via `openclaw.plugin.json` config or the OpenClaw UI:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `rtk_enabled` | boolean | `true` | Enable RTK command rewriting |
| `schema_compression_enabled` | boolean | `true` | Enable tool schema compression |
| `response_compression_enabled` | boolean | `true` | Enable tool response compression |
| `skip_tools` | string[] | `["Read", "read_file", "Glob", "list_directory", "NotebookRead", "read", "glob", "notebookread"]` | Tool names whose responses should not be compressed |
| `verbose` | boolean | `false` | Log detailed rewrite/compression info to console |

## Architecture

This plugin is a **thin delegate** — all heavy lifting is performed by the `rtk` and `tokenless` CLI binaries via subprocess calls. Timeout guards (2 s for rtk, 3 s for tokenless) ensure the plugin never blocks the agent loop, and any failure silently passes through the original data unmodified.
