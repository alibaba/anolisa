# Token-Less

[中文版](README_zh.md)

**LLM token optimization toolkit** — schema/response compression + command rewriting + tool environment readiness.

Token-Less combines complementary strategies to minimize LLM token consumption:

- **Schema & Response Compression** — Compresses OpenAI Function Calling tool definitions and API responses via the `tokenless-schema` library, cutting structural overhead before tokens ever reach the context window.
- **TOON Context Compression** — Encodes JSON responses to TOON (Token-Oriented Object Notation) format via the `toon` binary, reducing token usage by 15-40% for structured data.
- **Command Rewriting** — Integrates [RTK](https://github.com/rtk-ai/rtk) to filter and rewrite CLI command output, eliminating noise that would otherwise waste 60–90% of tokens.
- **Tool Ready** — Pre-checks tool execution environments (binaries, configs, permissions, network), auto-fixes missing dependencies, and classifies execution failures as environment issues vs logic errors — reducing wasted retry tokens.

Three integration paths are available:

- **OpenClaw plugin** — covers command rewriting, response compression, and schema compression in one plugin.
- **copilot-shell hook** — intercepts Shell commands via a PreToolUse hook and delegates to RTK for command rewriting + output filtering.
- **Hermes Agent plugin** — response compression, TOON encoding, command rewriting (block + suggest), and Tool Ready environment pre-check via Hermes's native plugin system.
- **Qoder CLI plugin** — Tool Ready, command rewriting, and response compression via Qoder's native hook system.
- **Claude Code plugin** — RTK command rewriting, response/TOON compression, and Tool Ready via Claude Code's official plugin marketplace.
- **Codex plugin** — response compression, TOON encoding, Tool Ready, and command rewriting via Codex's native hook system.

## Features

| Capability | Token Savings | Details |
|---|---|---|
| Schema compression | ~57% | Compresses OpenAI Function Calling tool schemas |
| Response compression | ~26–78% | Compresses API / tool responses (varies by content type) |
| Reversible compression (stash) | — | Dropped array items are stashed and retrievable via `<<tokenless:KEY>>` markers |
| TOON context compression | 15–40% | Encodes JSON to TOON format for LLMs |
| Command rewriting | 60–90% | Filters CLI output via RTK (70+ commands supported) |
| Tool Ready | reduces retry waste | Pre-check env, auto-fix deps, failure attribution |
| OpenClaw plugin | — | Command rewriting ✅, Response compression ✅, Schema compression ✅ |
| copilot-shell hooks | — | Tool Ready ✅, Command rewriting ✅, Response compression ✅, TOON ✅, Schema compression ✅ |
| Hermes Agent plugin | — | Tool Ready ✅, Command rewriting ✅, Response compression ✅, TOON ✅, Schema compression ⏳ |
| Qoder CLI plugin | — | Tool Ready ✅, Command rewriting ✅, Response compression ✅ |
| Claude Code plugin | — | Tool Ready ✅, Command rewriting ✅, Response compression ✅, TOON ✅ |
| Codex plugin | — | Tool Ready ✅, Command rewriting ✅, Response compression ✅, TOON ✅ |
| Zero runtime deps | — | Pure Rust, single static binary |

## Applicable Scenarios & Expected Effects

tokenless only removes redundancy from **tool call responses** before they enter the LLM context; it does not touch model reasoning or conversation history. The payoff depends heavily on the share and shape of tool responses in the session.

### Where it pays off

| Workload | Primary strategy | Why |
|----------|-----------------|-----|
| Shell-heavy (build/test/triage) | Command rewriting (RTK) | `cargo`/`npm`/`go`/`pytest` output carries lots of progress/warning noise; RTK cuts 60–90% |
| API/fetch-heavy (REST, web_fetch) | Response compression + TOON | JSON carries debug/null/empty and syntax overhead; 26–78% compression, TOON adds 15–40% |
| Agents with many tools | Schema compression | Many Function Calling definitions carry verbose descriptions; ~57% |
| Long responses that must stay faithful | Reversible compression (Stash) | Truncated content is `retrieve`-able end-to-end lossless; thresholds can be tightened safely |

### Where it pays little or doesn't apply

- **Chat-heavy / few tool calls**: tool-response share is tiny, overall savings approach 0.
- **Already-short responses**: when `after >= before`, the CLI emits the original and records no stats (expected).
- **Model inference tokens / billed tokens**: outside what tokenless touches.

### Estimating the effect

> The shares below are **illustrative estimates** that vary widely by task, not measured constants.

| Session component | Typical share | tokenless can optimize |
|-----------|-------------|----------------------|
| LLM reasoning output (text generation) | ~35% | ❌ Not involved |
| LLM input (system prompt + conversation history) | ~40% | ❌ Not involved |
| Tool call arguments | ~5% | ❌ Not involved |
| **Tool responses (API returns + command output)** | **~20%** | **✅ Optimization scope** |

**Actual savings rate = reported compression rate × tool response share**

Example: dashboard shows 60% compression rate, but if tool responses account for 20% of total consumption, the actual savings rate is 60% × 20% = **12%**. This is why savings feel "lighter than a feather" in experiments consuming 15 million tokens — tokenless only optimizes the ~3 million tokens of tool responses.

> Stash makes compression **end-to-end lossless**: you can tighten truncation thresholds for higher inline savings and recover the original via the `<<tokenless:KEY>>` marker when needed, with no correctness impact. Use `TOKENLESS_COMPRESSION_ENABLED=0/1` dual runs to compare real savings.
> See [user manual](docs/tokenless-user-manual-en.md) for per-strategy trigger conditions.

## Architecture

```
Token-Less/
├── crates/tokenless-schema/   # Core library: SchemaCompressor + ResponseCompressor
├── crates/tokenless-ccr/      # Reversible compression stash (Compress-Cache-Retrieve)
├── crates/tokenless-cli/      # CLI binary: `tokenless` command (env-check, compress, retrieve, stats)
├── adapters/tokenless/        # FHS adapter bundle (manifest, common, openclaw, hermes, qoder, claude-code, codex)
│   ├── manifest.json            # Adapter manifest (cosh + openclaw + hermes + qoder + claude-code + codex)
│   ├── common/                  # Shared: hooks, spec, env-fix, commands, cosh-extension
│   │   ├── hooks/               # copilot-shell hooks (tool-ready + rewrite + compression)
│   │   ├── cosh-extension.json  # copilot-shell extension manifest (references common/hooks/)
│   │   ├── tool-ready-spec.json # Tool dependency spec (4 categories)
│   │   ├── tokenless-env-fix.sh # Auto-fix script for missing deps
│   │   └── commands/            # Hook command configs
│   ├── openclaw/                # OpenClaw plugin + agent scripts
│   ├── hermes/                  # Hermes Agent plugin + scripts
│   ├── qoder/                   # Qoder CLI plugin + scripts
│   ├── claude-code/             # Claude Code plugin + marketplace + hooks
│   └── codex/                   # Codex plugin + scripts
├── third_party/rtk/           # RTK vendored source (justfile clone+patch from GitHub)
├── third_party/patches/      # Patches for vendored third_party sources
├── Makefile                   # Unified build system
└── scripts/                    # Helper scripts
```

## Quick Start

```bash
# Clone repo (no submodules needed)
git clone <repo-url>
cd Token-Less

# Full setup: build + install binaries + deploy all adapters
make setup
```

Both methods install `tokenless` to `~/.local/bin`, helper binaries `rtk`/`toon` alongside it, and deploy the adapters (hooks + OpenClaw plugin + Hermes plugin).

## CLI Usage

### compress-schema

Compress a single tool schema:

```bash
# From file
tokenless compress-schema -f tool.json

# From stdin
cat tool.json | tokenless compress-schema
```

Compress a batch of tools (JSON array):

```bash
tokenless compress-schema -f tools.json --batch
```

### compress-response

Compress an API response:

```bash
# From file
tokenless compress-response -f response.json

# From stdin
curl -s https://api.example.com/data | tokenless compress-response
```

By default `compress-response` stashes dropped array items so they can be
retrieved later (see [Reversible compression](docs/stash-reversible-compression.md)).
Pass `--no-stash` for lossy truncation, or `--stash-db <path>` to override the
stash database (default `~/.tokenless/stash.db`).

### retrieve

Recover a payload stashed during `compress-response`. Accepts a bare 24-hex
hash or any text containing a `<<tokenless:HASH>>` marker:

```bash
# Bare hash
tokenless retrieve c30ccf5ed1125e0ed871ba8e

# Or paste the whole truncation line — the hash is extracted automatically.
# (Use the FULL 24-hex hash from your output; the value below is shorthand.)
tokenless retrieve "<... 195 items truncated, retrieve with <<tokenless:c30ccf5ed1125e0ed871ba8e>>"
```

### compress-toon / decompress-toon

Encode JSON to TOON format (or decode back to JSON):

```bash
# Encode JSON to TOON
echo '{"name":"Alice","age":30}' | tokenless compress-toon
# name: Alice
# age: 30

# Decode TOON back to JSON
echo 'name: Alice\nage: 30' | tokenless decompress-toon
# {"name":"Alice","age":30}
```

## copilot-shell Hooks

The adapter provides hooks that are auto-discovered by copilot-shell via the cosh extension manifest:

| Hook | Event | File | Description |
|------|-------|------|-------------|
| Tool environment check | PreToolUse (all tools) | `tool_ready_hook.sh` | Pre-check env, auto-fix, skip-retry guidance |
| Command rewriting | PreToolUse (Shell) | `rewrite_hook.py` | Rewrite commands via RTK |
| Response compression + attribution + TOON | PostToolUse | `compress_response_hook.py` | Compress + env error attribution + TOON |
| Schema compression | BeforeModel | `compress_schema_hook.py` | Compress tool schemas |

### Install

```bash
make cosh-extension-install  # or: make openclaw-install, make hermes-install
```

Hooks are registered via the cosh extension manifest (`cosh-extension.json`) and auto-discovered by copilot-shell — no manual `settings.json` configuration needed.

## Tool Ready

Tool Ready prevents wasted LLM tokens from retrying commands that fail due to missing environment dependencies.

**How it works**: Before each tool call, the `tool_ready_hook.sh` hook checks the tool's dependency list (from `tool-ready-spec.json`). If dependencies are missing, it reports `NOT_READY` with "Skip retry" guidance so the LLM doesn't waste tokens retrying a command that can't succeed. After a tool call fails, the compression hook classifies the error (missing binary, permissions, network, etc.) and injects attribution context.

### env-check CLI

```bash
# Check a specific tool
tokenless env-check --tool Shell

# Check all tools
tokenless env-check --all

# Generate checklist
tokenless env-check --checklist

# Check and auto-fix missing deps
tokenless env-check --tool Shell --fix
```

### Configuration

Per-tool dependencies are declared in `tool-ready-spec.json` (shipped within the adapter bundle at `common/tool-ready-spec.json`):

```json
{
  "Shell": {
    "required": [
      { "binary": "jq", "package": "jq", "manager": "apt" }
    ],
    "recommended": [
      { "binary": "rtk", "version": ">=0.35", "package": "rtk", "manager": "cargo",
        "fallback": [
          { "method": "symlink", "binary": "rtk", "source": "/usr/libexec/anolisa/tokenless/rtk" }
        ]
      }
    ]
  }
}
```

String format `"jq"` is also supported (auto-converts to object).

## OpenClaw Plugin

The plugin hooks into the OpenClaw agent loop at two stages:

| Hook | Event | Action | Status |
|---|---|---|---|
| Command rewriting | `before_tool_call` | Rewrites `exec` commands to RTK equivalents for filtered output | ✅ Active |
| Response compression | `tool_result_persist` | Compresses tool results before they enter the context window | ✅ Active |
| Schema compression | — | Not supported by OpenClaw's hook system | ⏳ → ✅ |

**Response compression details:**
- Automatically compresses results from all tool types (`web_search`, `web_fetch`, `read_file`, etc.)
- Skips `exec` tool results when RTK is enabled — RTK already produces optimized output, avoiding double-compression
- Observed savings: **~78%** on `web_fetch` results, varies by content type

Each hook degrades gracefully — if the corresponding binary (`rtk` or `tokenless`) is not installed, that hook is silently skipped.

### Configuration

Options in `openclaw.plugin.json`:

| Option | Default | Description |
|---|---|---|
| `rtk_enabled` | `true` | Enable RTK command rewriting |
| `schema_compression_enabled` | `true` | Enable tool schema compression (pending OpenClaw support) |
| `response_compression_enabled` | `true` | Enable tool response compression via `tool_result_persist` |
| `verbose` | `true` | Log detailed rewrite/compression info |

## Hermes Agent Plugin

The plugin registers hooks at three Hermes events, covering five strategies:

| Strategy | Event | Action | Status |
|---|---|---|---|
| Tool Ready | `pre_tool_call` | Environment readiness pre-check with auto-fix and skip-retry feedback | ✅ Active |
| Command rewriting | `pre_tool_call` | Blocks original command, suggests `rtk`-rewritten version (one extra round-trip) | ✅ Active |
| Response compression | `transform_tool_result` | Compresses tool results via `tokenless compress-response` | ✅ Active |
| TOON encoding | `transform_tool_result` | Pipeline step after response compression — encodes JSON to TOON format | ✅ Active |
| Session tracking | `on_session_start` | Propagates agent/session IDs for stats recording | ✅ Active |
| Schema compression | — | Not supported by Hermes hook system (no hook exposes tool schemas) | ⏳ Blocked |

**How command rewriting works in Hermes**: Hermes's `pre_tool_call` hook can only block tool execution (not modify arguments), so the plugin blocks the original shell command and returns a message suggesting the RTK-rewritten version. The agent then re-executes with the optimized command, adding one extra tool-call round-trip. This is safe — `rtk rewrite` only does text substitution and never executes the command.

Each hook degrades gracefully — if the corresponding binary is not installed, that hook is silently skipped.

### Install

```bash
make hermes-install
```

Enable the plugin:

```bash
hermes plugins enable tokenless
```

Or add to `~/.hermes/config.yaml`:

```yaml
plugins:
  enabled:
    - tokenless
```

## Qoder CLI Plugin

The plugin registers hooks at three Qoder events, covering three strategies:

| Strategy | Event | Action | Status |
|---|---|---|---|
| Tool Ready | `PreToolUse` | Environment readiness pre-check with auto-fix and skip-retry feedback | ✅ Active |
| Command rewriting | `PreToolUse` | Rewrites shell commands via RTK for token savings | ✅ Active |
| Response compression | `PostToolUse` | Compresses tool responses and encodes to TOON format | ✅ Active |

Each hook degrades gracefully — if the corresponding binary is not installed, that hook is silently skipped.

### Install

```bash
make qoder-install
```

## Claude Code Plugin

The plugin registers hooks at two Claude Code events, covering four strategies:

| Strategy | Event | Action | Status |
|---|---|---|---|
| Tool Ready | `PreToolUse` | Environment readiness pre-check with auto-fix and skip-retry feedback | ✅ Active |
| Command rewriting | `PreToolUse` (Bash) | Rewrites shell commands via RTK for token savings | ✅ Active |
| Response compression | `PostToolUse` | Compresses tool responses and encodes to TOON format | ✅ Active |
| TOON encoding | `PostToolUse` | Pipeline step after response compression — encodes JSON to TOON format | ✅ Active |

Claude Code v2 requires plugins to be sourced from a registered marketplace. We expose the adapter's `claude-code/` directory as a single-plugin marketplace (`anolisa-tokenless`), then install `tokenless@anolisa-tokenless` from it. The marketplace name is component-scoped so multiple ANOLISA components can each register their own without colliding.

### Install

```bash
make claude-code-install
```

## Codex Plugin

The plugin registers hooks at four Codex events, covering four strategies:

| Strategy | Event | Action | Status |
|---|---|---|---|
| Session check | `SessionStart` | Verifies tokenless CLI is installed and functional (non-blocking) | ✅ Active |
| Tool Ready | `PreToolUse` | Environment readiness pre-check with auto-fix and skip-retry feedback | ✅ Active |
| Command rewriting | `PreToolUse` | Rewrites shell commands via RTK for token savings | ✅ Active |
| Response compression | `PostToolUse` | Compresses tool responses and encodes to TOON format, injects compressed summary as `additionalContext` | ✅ Active |

> **Codex Protocol Constraint**: PostToolUse hooks cannot suppress the original tool output. The plugin injects a compressed *summary* as `additionalContext` — the model sees both the original output and the compressed summary.

### Install

```bash
make codex-install
```

## Build

| Target | Description |
|---|---|
| `make build` | Build `tokenless` + `rtk` + `toon` (release mode) |
| `make build-tokenless` | Build `tokenless` + `rtk` (via justfile) |
| `make build-toon` | Install TOON binary via `cargo install toon-format` |
| `make install` | Build and install binaries to `BIN_DIR` (default: ~/.local/bin) |
| `make test` | Run all tests (Rust + hooks) |
| `make test-hooks` | Run hook integration tests |
| `make lint` | Run clippy checks |
| `make fmt` | Format code |
| `make clean` | Clean build artifacts |
| `make adapter-install` | Install all adapters (cosh + openclaw + hermes) |
| `make adapter-uninstall` | Remove all adapters |
| `make cosh-extension-install` | Install Copilot Shell extension |
| `make cosh-extension-uninstall` | Remove Copilot Shell extension |
| `make openclaw-install` | Install OpenClaw plugin |
| `make openclaw-uninstall` | Remove OpenClaw plugin |
| `make hermes-install` | Install Hermes Agent plugin |
| `make hermes-uninstall` | Remove Hermes Agent plugin |
| `make qoder-install` | Install Qoder CLI plugin |
| `make qoder-uninstall` | Remove Qoder CLI plugin |
| `make claude-code-install` | Install Claude Code plugin |
| `make claude-code-uninstall` | Remove Claude Code plugin |
| `make codex-install` | Install Codex plugin |
| `make codex-uninstall` | Remove Codex plugin |
| `make setup` | Full setup: build + install + all adapters |

Override install paths:

```bash
make install BIN_DIR=/usr/local/bin
```

## Project Structure

| Path | Description |
|---|---|
| `crates/tokenless-cli/` | CLI binary — `tokenless` command (compress, stats, env-check) |
| `crates/tokenless-schema/` | Core Rust library — `SchemaCompressor` and `ResponseCompressor` |
| `adapters/tokenless/` | FHS adapter bundle — manifest, env-check spec/fix, hooks, OpenClaw plugin |
| `adapters/tokenless/hermes/` | Hermes Agent adapter — plugin + detect/install/uninstall scripts |
| `adapters/tokenless/qoder/` | Qoder CLI adapter — plugin + detect/install/uninstall scripts |
| `adapters/tokenless/claude-code/` | Claude Code adapter — marketplace + plugin + hooks dispatcher |
| `adapters/tokenless/codex/` | Codex adapter — plugin + Python hook scripts |
| `third_party/rtk/` | RTK vendored source — command rewriting engine (justfile clone+patch) |
| `third_party/patches/` | Patches for vendored third_party sources |
| `Makefile` | Unified build system for the entire workspace |

## Prerequisites

- **Rust** toolchain >= 1.89 — required by rtk (edition 2024) and toon-format (is_multiple_of). Install via [rustup](https://rustup.rs)
- **just** — build runner for rtk setup (clone + patch orchestration)
- **Git** — for rtk source download via justfile

## License

Apache License 2.0 — see [LICENSE](LICENSE).
