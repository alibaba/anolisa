# Hook System

The cosh-core hook system allows injecting external logic at key points in the Agent execution flow. Hooks are implemented by executing external commands and support interception, auditing, and context injection.

## Event Points

| Event | Trigger Timing | Interceptable |
|-------|---------------|---------------|
| `PreToolUse` | Before tool execution | Yes (block/allow/ask) |
| `PostToolUse` | After tool execution | Yes (block/allow) |
| `PostToolUseFailure` | After tool execution failure | No |
| `UserPromptSubmit` | When user message is submitted | Yes (block/allow) |
| `SessionStart` | After session initialization | No |
| `Stop` | When Agent decides to stop | Yes (block/allow) |
| `BeforeModel` | Before LLM request is sent | No |
| `AfterModel` | After LLM response is received | No |

## Configuration

Define hooks in `~/.copilot-shell/config.toml`:

```toml
[hooks]
enabled = true

[[hooks.PreToolUse]]
name = "security-check"
command = "/usr/local/bin/my-security-hook"
timeout = 5000

[[hooks.SessionStart]]
name = "context-loader"
command = "/usr/local/bin/load-context"
timeout = 3000
```

Hooks can also be registered via an extension's `cosh-extension.json`.

## Protocol

### Input (stdin → hook process)

Core writes event data in JSON format to the hook process stdin:

```json
{
  "session_id": "abc-123",
  "cwd": "/home/user/project",
  "hook_event_name": "PreToolUse",
  "timestamp": "2026-07-01T10:00:00Z",
  "transcript_path": "/home/user/.copilot-shell/sessions/abc-123",
  "tool_name": "shell",
  "tool_input": { "command": "rm -rf /tmp/old" }
}
```

### Output (hook process → stdout)

Hooks return decisions in JSON format:

```json
{
  "decision": "block",
  "reason": "Dangerous rm -rf command",
  "systemMessage": "Command blocked by security policy"
}
```

### Decision Values

| decision | Meaning |
|----------|---------|
| `allow` | Allow to proceed |
| `block` / `deny` | Intercept, abort the operation |
| `ask` | Require user confirmation |
| None / empty | Pass-through (no intervention) |

### Additional Fields

| Field | Description |
|-------|-------------|
| `reason` | Decision reason (embedded in block/deny decisions, also serves as notification message fallback) |
| `systemMessage` | Notification message (displayed to user with priority over reason) |
| `hookSpecificOutput` | Custom JSON data (`additional_context` within it is injected into conversation context) |

### BeforeModel: rewriting tool declarations

`BeforeModel` input carries the full tool declarations for this request at
`llm_request.config.tools`. A hook may return a rewritten array at the same path,
for example to compress descriptions and JSON Schemas:

```json
{
  "hookSpecificOutput": {
    "llm_request": {
      "config": {
        "tools": [
          { "name": "shell", "description": "compressed", "parameters": { "type": "object" } }
        ]
      }
    }
  }
}
```

Constraints:

- The rewrite applies to that single LLM request only; the tool registry and the
  next turn's declarations are untouched
- Tool count, order, and names must match the input exactly, and `parameters`
  must be a JSON object; otherwise the whole array is discarded and the original
  declarations are used (tool filtering is not part of this protocol)
- The rewrite may only **compress**: an array whose estimated token count exceeds
  the original is rejected even when it is otherwise structurally valid. The
  context budget for the turn is computed from the original declarations before
  the hook runs, so a larger array would make the runtime under-account the
  request and risk a provider context overflow
- With multiple hooks the last valid array in configuration order wins; an
  invalid value never overwrites an earlier valid one
- The tool declaration subtree is exempt from key-based redaction, so schema
  properties named `api_key` or `token` are not corrupted

## Execution Model

1. Multiple hooks can be registered for the same event point, executed sequentially in configuration order
2. If any hook returns `block`, the final decision is block (short-circuit)
3. Hook timeout (default 5000ms) is treated as pass-through
4. Non-zero exit code from hook process is treated as error, does not affect main flow
5. Hook definitions without a `name` field are skipped

## Notifications

Notifications generated after hook execution are delivered to Shell via JSONL output:

```json
{"type":"stream_event","event":{"subtype":"hook_notification","hook_name":"security-check","message":"Command blocked by security policy","decision":"block"}}
```

Shell is responsible for rendering notification cards.

## Extension Hooks

Hooks registered via `cosh-extension.json` are merged with configuration file hooks and use the same execution protocol. See [extensions.md](extensions.md).
