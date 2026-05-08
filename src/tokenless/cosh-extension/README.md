# Token-Less Cosh Extension

Token optimization hooks for copilot-shell via the cosh extension format.
Intercepts and optimizes LLM interactions for **significant token savings**.

## Features

| Feature | Hook Event | Hook Script | Savings |
|---------|-----------|-------------|---------|
| Command rewriting (RTK) | PreToolUse (matcher: `Shell`) | `rewrite_hook.py` | 60–90% |
| Response compression → TOON | PostToolUse | `compress_response_hook.py` | 30–60% (combined) |
| Schema compression | BeforeModel | `compress_schema_hook.py` | ~57% |
| Standalone TOON encoding | PostToolUse | `compress_toon_hook.py` | 40–70% |

## Extension Format

This extension uses the `cosh-extension.json` manifest with `${extensionPath}`
for hook command paths. Copilot-shell discovers and loads the extension
automatically from standard extension directories — no manual `settings.json`
patching required.

The manifest also includes `description` fields on each hook for `/hooks` listing.

## Context Injection

The `COPILOT.md` file is injected into the model's context by copilot-shell.
It provides the agent with instructions for parsing TOON-encoded responses and
understanding compression annotations like `[tokenless]`.

## How It Works

### Command Rewriting (`rewrite_hook.py`)

1. copilot-shell fires `PreToolUse` before every `Shell` tool call.
2. The hook reads the JSON payload from stdin.
3. Delegates to `rtk rewrite` — the single source of truth for all rewrite rules.
4. Returns a JSON response with `hookSpecificOutput.tool_input` containing the rewritten command.

### Response Compression → TOON Pipeline (`compress_response_hook.py`)

The response compression hook runs a **sequential pipeline**:

1. copilot-shell fires `PostToolUse` after every tool call completes.
2. The hook reads the JSON payload from stdin (includes `tool_response`).
3. **Step 1 — Response Compression**: via `tokenless compress-response`:
   - Removes debug fields (debug, trace, stack, logs)
   - Removes null values and empty objects/arrays
   - Truncates long strings (>512 chars) and large arrays (>16 items)
4. **Step 2 — TOON Encoding** (if compressed result is valid JSON and `toon` is installed):
   - Encodes the compressed JSON to TOON format via `toon -e`
   - Eliminates JSON syntax overhead (quotes, commas, braces)
5. Returns a JSON response with `suppressOutput: true` and the compressed content as `additionalContext`.

```
Original JSON ──▶ Response Compression ──▶ TOON Encoding ──▶ Agent
                    (strip noise)            (format)
```

### Schema Compression (`compress_schema_hook.py`)

1. copilot-shell fires `BeforeModel` before each LLM request.
2. The hook reads the JSON payload from stdin (includes `llm_request`).
3. Compresses tool schemas via `tokenless compress-schema --batch`.
4. Returns a JSON response with the compressed `tools` array.

### Standalone TOON Encoding (`compress_toon_hook.py`)

Pure TOON-only encoding without response compression. Use this if you only want
TOON format conversion. The combined pipeline (`compress_response_hook.py`) is
recommended for maximum savings.

All hooks are **fail-open**: if dependencies are missing or processing fails,
the original data passes through unchanged (output `{}`).

## Prerequisites

| Dependency | Version   | Required |
|------------|-----------|----------|
| rtk        | >= 0.35.0 | Yes (for command rewriting) |
| toon       | any       | Recommended (for TOON encoding step) |
| tokenless  | any       | Yes (for response/schema compression) |
| python3    | >= 3.9    | Yes |

> **Note:** `jq` is no longer a runtime dependency for hooks (Python handles JSON parsing internally).

## Installation

### RPM (system-wide)

The RPM package installs the extension to `/usr/share/anolisa/extensions/tokenless/`.
Copilot-shell auto-discovers system extensions — no additional configuration needed.

### Via install script

```bash
# Copies extension to user's copilot-shell extensions directory
bash /usr/share/tokenless/scripts/install.sh --cosh
```

### Via Makefile (local build)

```bash
make cosh-install
```

### Manual (user-level)

```bash
mkdir -p ~/.copilot-shell/extensions/tokenless
cp -r cosh-extension/* ~/.copilot-shell/extensions/tokenless/
```

Copilot-shell will auto-discover the extension and register its hooks.

> **Note:** For qwen-code, use `~/.qwen-code/extensions/tokenless/` instead.

### Manual (system-level)

```bash
mkdir -p /usr/share/anolisa/extensions/tokenless/hooks
mkdir -p /usr/share/anolisa/extensions/tokenless/commands
cp cosh-extension/cosh-extension.json /usr/share/anolisa/extensions/tokenless/
cp cosh-extension/COPILOT.md /usr/share/anolisa/extensions/tokenless/
cp cosh-extension/hooks/*.py /usr/share/anolisa/extensions/tokenless/hooks/
cp cosh-extension/commands/*.toml /usr/share/anolisa/extensions/tokenless/commands/
```

## Slash Commands

| Command | Description |
|---------|-------------|
| `/tokenless-stats` | Show compression stats summary |

## Hook Management

| Command | Description |
|---------|-------------|
| `/hooks list` | List all active hooks (shows tokenless hooks with descriptions) |
| `/hooks disable tokenless-rewrite` | Disable command rewriting for current session |
| `/hooks enable tokenless-rewrite` | Re-enable command rewriting |

## Verification

Test each hook manually:

```bash
# Command rewriting
echo '{"tool_input":{"command":"cargo test"}}' | python3 hooks/rewrite_hook.py

# Response compression → TOON pipeline
echo '{"tool_name":"Shell","tool_response":"{\"users\":[{\"id\":1,\"name\":\"Alice\"},{\"id\":2,\"name\":\"Bob\"}],\"debug\":\"info\"}"}' | python3 hooks/compress_response_hook.py

# Schema compression
echo '{"llm_request":{"tools":[{"name":"test","description":"A test tool","parameters":{}}]}}' | python3 hooks/compress_schema_hook.py

# Standalone TOON encoding
echo '{"tool_name":"Shell","tool_response":"{\"users\":[{\"id\":1,\"name\":\"Alice\"}]"}' | python3 hooks/compress_toon_hook.py
```

## Troubleshooting

| Problem | Solution |
|---------|----------|
| Extension not loaded | Verify extension dir path and restart copilot-shell |
| `python3 not found` warning | Install python3 >= 3.9 |
| `rtk too old` warning | Upgrade: `cargo install rtk` |
| `tokenless not installed` warning | Build and install: `make install` |
| Response not compressed | Responses shorter than 200 bytes are skipped |
| TOON step skipped | Install toon: `make build-toon && make install` |
| Schema compression not active | Expected — waiting for anolisa protocol to add `tools` to LLMRequest |
| `jq` still in settings.json | Legacy hooks; run `install.sh --cosh` to migrate |