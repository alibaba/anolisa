# Tokenless Plugin for Codex

Intelligent tool response compression and environment error detection plugin for
[Codex](https://github.com/openai/codex). Reduces token consumption by stripping
noise, truncating verbose output, and classifying environment errors with
actionable fix hints.

## Features

| Feature | Description |
|---------|-------------|
| **Response Compression** | Strips debug fields, nulls, empty values; truncates long strings (512 chars) and arrays (16 items); limits depth to 8 levels |
| **TOON Encoding** | Further compresses valid JSON responses (15-40% additional savings vs. compressed JSON) |
| **Environment Error Detection** | Classifies tool failures as dependency/permission/file/network/package issues; injects fix hints to prevent retry loops |
| **Statistics Tracking** | Records every compression operation to SQLite for auditing and optimization |

## How It Works

The plugin registers four hooks with Codex:

1. **`SessionStart`** — verifies the `tokenless` CLI is installed and functional (non-blocking)
2. **`PreToolUse` (tool-ready)** — runs before every tool execution:
   - Checks if required dependencies are available via `tokenless env-check`
   - Auto-installs missing dependencies via `tokenless env-check --fix`
   - Blocks the tool if critical dependencies are still missing after auto-fix
3. **`PreToolUse` (rewrite)** — runs before shell command execution:
   - Rewrites commands via `rtk rewrite` for token optimization
   - Only applies to Bash/Shell/terminal/programmatic tools
4. **`PostToolUse`** — runs after every tool execution:
   - Skips content-reading tools (Read, Glob) and small responses (< 500 chars)
   - Classifies environment errors and injects fix hints
   - Compresses large JSON responses via `tokenless compress-response`
   - Applies TOON encoding for additional savings
   - Injects a compressed summary as `additionalContext`

> **Codex Protocol Constraint**: PostToolUse hooks cannot suppress the original
> tool output (`suppressOutput` is rejected). The plugin therefore injects a
> compressed *summary* as `additionalContext` — the model sees both the original
> output and the compressed summary, and can use the summary for efficient
> processing of large outputs.

## Installation

### Prerequisites

- Rust toolchain ≥ 1.89.0
- The `anolisa` source tree (this repository)

### Install

```bash
cd src/tokenless/adapters/tokenless/codex
./scripts/install.sh
```

This builds the `tokenless` Rust CLI in release mode and installs it to
`~/.local/bin/`. Add `~/.local/bin` to your `PATH` if not already present.

### Configure Codex

Add the plugin to your Codex `config.toml`:

```toml
[plugins]
tokenless = { enabled = true }
```

Or install from the marketplace (once published).

### Verify

```bash
./scripts/detect.sh
# {"installed": true, "version": "1.0.0", "path": "/home/user/.local/bin/tokenless"}
```

## Hook Output Format

The plugin injects `additionalContext` in the following format:

```
[tokenless:compressed] Bash: 45,230 → 2,100 chars (95% reduction)
[tokenless:env:ENV_DEPENDENCY_MISSING] Missing dependency detected. ...
Do NOT retry the same command — fix the environment first.
--- compressed content ---
{... compressed JSON or TOON content ...}
--- end compressed content ---
```

## Configuration

Configuration is managed through the `tokenless` CLI's environment and config file:

| Variable | Default | Description |
|----------|---------|-------------|
| `TOKENLESS_AGENT_ID` | `codex` | Agent identifier for statistics |
| `TOKENLESS_BIN` | (auto-detected) | Path to tokenless binary |
| `TOKENLESS_STATS_ENABLED` | `1` | Enable statistics recording |
| `TOKENLESS_STATS_DB` | `~/.tokenless/stats.db` | Statistics database path |

### View Statistics

```bash
tokenless stats summary
tokenless stats list --limit 20
tokenless stats show <id>
```

## Compression Pipeline

```
Raw tool_response (JSON)
    │
    ├─ Step 1: ResponseCompressor
    │   ├─ Drop: debug, trace, stack, stacktrace, logs, logging fields
    │   ├─ Truncate: strings > 512 chars, arrays > 16 items
    │   ├─ Drop: null values, empty strings, empty arrays, empty objects
    │   └─ Limit: max depth 8 levels
    │
    ├─ Step 2: TOON Encoding (if result is still valid JSON)
    │   └─ Binary-to-TOON format encoding (~15-40% additional savings)
    │
    └─ Guard: output only if smaller than input (safety check)
```

## Architecture

```
codex-plugin-tokenless/
├── plugin.json.in           # Codex plugin manifest (version-stamped by Makefile)
├── hooks/
│   └── hooks.json           # Hook definitions (SessionStart, PreToolUse, PostToolUse)
├── scripts/
│   ├── compress-response    # PostToolUse: response compression + env error detection
│   ├── rewrite-hook         # PreToolUse: RTK command rewriting
│   ├── tool-ready           # PreToolUse: environment check + auto-fix
│   ├── check-tokenless      # SessionStart: version/availability check
│   ├── install.sh           # Build and install tokenless CLI + register plugin
│   ├── detect.sh            # Detect tokenless availability
│   └── uninstall.sh         # Cleanup plugin registration + marketplace
└── README.md
```

## Related

- [Tokenless Rust CLI](../../crates/tokenless-cli/) — core compression engine
- [OpenClaw Plugin](../openclaw/) — same compression for OpenClaw
- [Hermes Plugin](../hermes/) — same compression for Hermes

## License

Same as the ANOLISA project.
