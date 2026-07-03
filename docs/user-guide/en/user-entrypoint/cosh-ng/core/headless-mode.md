# Headless Mode

cosh-core's headless mode provides a JSONL protocol interface via stdin/stdout. It is the standard communication method between cosh-shell and cosh-core, and can also be integrated by any client.

## Startup

```bash
# Long-running mode (continuously receives JSONL)
cosh-core --headless

# Single prompt (auto-exits after execution)
cosh-core --headless "Check disk usage"

# With parameters
cosh-core --headless --model qwen-max --approval-mode trust
```

## Input Messages (Shell → Core)

All input is sent line by line via stdin, with each line being a JSON object distinguished by the `type` field.

### user — User Message

```json
{
  "type": "user",
  "message": { "role": "user", "content": "List files in current directory" },
  "session_id": "optional-session-id",
  "shell_context": { "cwd": "/home/user/project", "env": {}, "last_exit_code": 0 }
}
```

### control_request — Control Request

```json
{
  "type": "control_request",
  "request_id": "req-001",
  "request": { "subtype": "initialize" }
}
```

Supported `subtype` values:

| subtype | Description |
|---------|-------------|
| `initialize` | Initialize session (returns capability declaration + system init message) |
| `interrupt` | Interrupt current generation |
| `shutdown` | Shut down process |
| `switch_model` | Switch model (requires `model` field) |
| `reload_config` | Reload config.toml |
| `config_override` | Runtime config override (`approval_mode`, `allowed_tools`) |

### control_response — Control Response (replying to Core's request)

```json
{
  "type": "control_response",
  "response": {
    "subtype": "tool_approval",
    "request_id": "req-002",
    "response": { "behavior": "allow" }
  }
}
```

### registry_request — Registry Request

```json
{
  "type": "registry_request",
  "request_id": "reg-001",
  "domain": "tools",
  "action": "list",
  "params": {}
}
```

## Output Messages (Core → Shell)

All output is written to stdout, also line-by-line JSON.

### system — System Message

```json
{"type":"system","subtype":"init","session_id":"...","model":"qwen3.7-plus","tools":["ask_user_question","edit","grep","read_file","shell","skill","todo","write_file"]}
```

Common `subtype` values: `init`, `auth_required`, `auth_ok`, `model_switched`, `config_reloaded`.

### stream_event — Streaming Event

Token-by-token output during LLM generation:

```json
{"type":"stream_event","event":{"subtype":"text_delta","content":"Hello"}}
{"type":"stream_event","event":{"subtype":"thinking_delta","content":"Let me analyze..."}}
{"type":"stream_event","event":{"subtype":"tool_use_begin","tool_name":"shell","tool_use_id":"tu-1"}}
{"type":"stream_event","event":{"subtype":"tool_use_delta","content":"{\"command\":\"df -h\"}"}}
{"type":"stream_event","event":{"subtype":"tool_use_end"}}
```

### control_request — Core-initiated Request

When Core needs Shell cooperation (e.g., tool approval, user question):

```json
{
  "type": "control_request",
  "request_id": "cr-001",
  "request": { "subtype": "can_use_tool", "tool_name": "shell", "tool_input": {"command": "rm -rf /tmp/old"} }
}
```

### result — Turn End

```json
{"type":"result","subtype":"success","is_error":false,"result":"completed","session_id":"...","duration_ms":1234}
```

## Initialization Handshake

Standard startup sequence:

```
Shell → Core:  {"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}
Core → Shell:  {"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{...}}}}
Core → Shell:  {"type":"system","subtype":"init","session_id":"...","model":"...","tools":[...]}
```

`capabilities` declares the protocol capabilities Core supports:

| Capability | Description |
|-----------|-------------|
| `can_handle_can_use_tool` | Supports tool approval protocol |
| `can_handle_host_executed_shell_tool_result` | Supports Shell-side execution result callback |
| `can_handle_shell_evidence_tool` | Supports terminal evidence tool |

## Authentication Flow

If no API key is configured, Core sends an authentication request during initialization:

```
Core → Shell:  {"type":"control_request","request_id":"auth-init","request":{"subtype":"auth_required","reason":"not_configured","providers":[...]}}
Shell → Core:  {"type":"control_response","response":{"subtype":"auth_response","request_id":"auth-init","response":{"provider_id":"dashscope","values":{"api_key":"sk-xxx"},"persist":true}}}
Core → Shell:  {"type":"system","subtype":"auth_ok"}
```

## Session Resume

```bash
cosh-core --headless --resume <session-id>
```

Core loads history messages from `~/.copilot-shell/sessions/<session-id>.json` and restores the conversation context.

## Error Handling

Protocol-level errors are delivered via `result` messages:

```json
{"type":"result","is_error":true,"errors":["provider returned HTTP 429: rate limit exceeded"],"session_id":"..."}
```

Input lines with JSON parse failures are silently ignored (debug log only) and do not terminate the process.
