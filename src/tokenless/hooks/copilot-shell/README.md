# Token-Less copilot-shell Hooks

Intercept and optimize LLM interactions via copilot-shell hooks for **significant token savings**.

## Features

| Feature | Hook Event | Status | Savings |
|---------|-----------|--------|---------|
| Command rewriting (RTK) | PreToolUse | ✅ Fully available | 60–90% |
| Response compression → TOON | PostToolUse | ✅ Fully available | 30–60% (combined) |
| Schema compression | BeforeModel | ⏳ Placeholder (waiting for anolisa protocol to include `tools` in LLMRequest) | ~57% |

## How It Works

### Command Rewriting (`tokenless-rewrite.sh`)

1. copilot-shell fires `PreToolUse` before every `Shell` tool call.
2. The hook reads the JSON payload from stdin (`{ "tool_input": { "command": "..." } }`).
3. Delegates to `rtk rewrite` — the single source of truth for all rewrite rules.
4. Returns a JSON response with `hookSpecificOutput.tool_input` containing the rewritten command.

### Response Compression → TOON Pipeline (`tokenless-compress-response.sh`)

The response compression hook now runs a **sequential pipeline**:

1. copilot-shell fires `PostToolUse` after every tool call completes.
2. The hook reads the JSON payload from stdin (includes `tool_response`).
3. **Step 1 — Response Compression**: via `tokenless compress-response`:
   - Removes debug fields (debug, trace, stack, logs)
   - Removes null values and empty objects/arrays
   - Truncates long strings (>512 chars) and large arrays (>16 items)
4. **Step 2 — TOON Encoding** (if compressed result is valid JSON and `toon` is installed):
   - Encodes the compressed JSON to TOON format via `toon -e`
   - Eliminates JSON syntax overhead (quotes, commas, braces)
5. Returns a JSON response with `suppressOutput: true` and the combined compressed content as `additionalContext`.

```
Original JSON ──▶ Response Compression ──▶ TOON Encoding ──▶ Agent
                    (strip noise)            (format)
```

> **Note:** TOON encoding only applies if the response-compressed result is still valid JSON. If compression produces non-JSON output (e.g., truncated marker breaks JSON), TOON is skipped and the response-compressed JSON is returned directly.

### Schema Compression (`tokenless-compress-schema.sh`)

1. copilot-shell fires `BeforeModel` before each LLM request.
2. The hook reads the JSON payload from stdin (includes `llm_request`).
3. Compresses tool schemas via `tokenless compress-schema --batch`.
4. Returns a JSON response with the compressed `tools` array.

> **Note:** Schema compression is currently a functional placeholder. The anolisa copilot-shell protocol does not yet include `tools` in the decoupled `LLMRequest` type. The hook will activate automatically once the protocol is extended — no code changes required.

All hooks are **fail-open**: if dependencies are missing or processing fails, the original data passes through unchanged.

## Prerequisites

| Dependency | Version   | Required |
|------------|-----------|----------|
| rtk        | >= 0.23.0 | Yes (for command rewriting) |
| toon       | any       | Recommended (for TOON encoding step) |
| jq         | any       | Yes |
| tokenless  | any       | Yes (for response compression) |

## Installation

### Automatic

```bash
make copilot-shell-install
```

### Manual

1. Copy the hook scripts:
```bash
mkdir -p /usr/share/tokenless/hooks/copilot-shell
cp hooks/copilot-shell/tokenless-*.sh /usr/share/tokenless/hooks/copilot-shell/
chmod +x /usr/share/tokenless/hooks/copilot-shell/tokenless-*.sh
```

2. Add the following to your settings file (`~/.copilot-shell/settings.json` or `~/.qwen-code/settings.json`):
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Shell",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-rewrite.sh",
            "name": "tokenless-rewrite",
            "timeout": 5000
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-compress-response.sh",
            "name": "tokenless-compress-response",
            "timeout": 10000
          }
        ]
      }
    ],
    "BeforeModel": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-compress-schema.sh",
            "name": "tokenless-compress-schema",
            "timeout": 10000
          }
        ]
      }
    ]
  }
}
```

The `tokenless-compress-response.sh` hook now includes TOON encoding as a second pass — no separate TOON hook is needed.

## Verification

Test each hook manually:

```bash
# Command rewriting
echo '{"tool_input":{"command":"cargo test"}}' | bash hooks/copilot-shell/tokenless-rewrite.sh

# Response compression → TOON pipeline
echo '{"tool_name":"Shell","tool_response":"{\"users\":[{\"id\":1,\"name\":\"Alice\"},{\"id\":2,\"name\":\"Bob\"}],\"debug\":\"info\"}"}' | bash hooks/copilot-shell/tokenless-compress-response.sh

# Schema compression (currently no-op until protocol adds tools support)
echo '{"llm_request":{"tools":[{"name":"test","description":"A test tool","parameters":{}}]}}' | bash hooks/copilot-shell/tokenless-compress-schema.sh
```

## Token Savings Examples

| Original Command       | Rewritten          | Typical Savings |
|------------------------|--------------------|-----------------|
| `cargo build`          | `rtk build`        | ~70%            |
| `cargo test`           | `rtk test`         | ~80%            |
| `npm run build`        | `rtk build`        | ~65%            |
| `go test ./...`        | `rtk test`         | ~75%            |
| `python -m pytest`     | `rtk test`         | ~85%            |
| `git diff --stat`      | `rtk diff --stat`  | ~60%            |

## Compression Pipeline Examples

| Input | Response Compression | + TOON | Combined Savings |
|-------|---------------------|--------|-----------------|
| `{"data":"ok","debug":"x","trace":"y","null":null}` | `{"data":"ok"}` (50%) | N/A (too small) | 50% |
| Large JSON with debug fields | Strips noise, truncates | Encodes clean JSON | 40–60% |
| Tabular JSON array `[{"id":1,...},{"id":2,...}]` | Keeps structure, removes nulls | Table format `users[2]{id,name}:` | 50–70% |

## Troubleshooting

| Problem | Solution |
|---------|----------|
| Hook not firing | Verify `settings.json` path and restart copilot-shell |
| `jq not installed` warning | Install jq: `brew install jq` (macOS) or `apt install jq` (Linux) |
| `rtk too old` warning | Upgrade: `cargo install rtk` |
| Command not rewritten | Not all commands have RTK equivalents — check `rtk rewrite "cmd"` directly |
| `tokenless not installed` warning | Build and install: `make install` |
| Response not compressed | Responses shorter than 200 bytes are skipped (not worth compressing) |
| TOON step skipped | Install toon: `make build-toon && make install`. Response compression still works without toon. |
| Schema compression not active | Expected — waiting for anolisa protocol to add `tools` to LLMRequest |
| JSON parse error | Ensure the settings JSON is valid — use `jq . < settings.json` to validate |
