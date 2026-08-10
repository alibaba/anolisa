# Hooks

[中文版](../../../../zh/user-entrypoint/cosh-ng/core/hooks.md)

Hooks run a command around an Agent event. Use them for policy checks,
notifications, or extra context, and enable them only from a source you trust.

## Enable and manage Hooks

Define them in `~/.copilot-shell/config.toml` or a trusted project config:

```toml
[hooks]
enabled = true

[[hooks.PreToolUse]]
name = "security-check"
command = "/usr/local/bin/my-security-hook"
matcher = "shell"
timeout = 60000
```

In the interactive terminal:

```text
/hooks
/hooks history
/hooks trust-project
/hooks enable <id>
/hooks disable <id>
```

Project Hooks do not run until the project root is trusted. Use
`/hooks untrust-project` to remove that trust. Shell Hook state is session-local;
Agent Hook state is persisted by the registry.

## Event names

| Event | When it runs | Can block? |
|---|---|---|
| `PreToolUse` | Before a tool call | Yes |
| `PostToolUse` | After a successful tool call | Yes |
| `PostToolUseFailure` | After a failed tool call | No |
| `UserPromptSubmit` | When a prompt is submitted | Yes |
| `SessionStart` | After session initialization | No |
| `Stop` | When the Agent stops | Yes |
| `BeforeModel` / `AfterModel` | Around a model request | No |

Use `matcher` to limit tool events. Hook commands receive one JSON object on
stdin. The object includes `hook_event_name`, `session_id`, `cwd`, and event
data such as `tool_name` and `tool_input`.

## Return a decision

Write one JSON object to stdout:

```json
{
  "decision": "block",
  "reason": "Dangerous command",
  "systemMessage": "Command blocked by security policy"
}
```

`allow` continues, `block`/`deny` stops the operation, `ask` requests user
confirmation, and an empty response passes through. Exit code `2` also blocks;
other non-zero exits are warnings. The default timeout is 60 seconds; set a
shorter `timeout` when a check must be quick. Use `sequential = true` when
multiple Hooks for an event must run in order.

## Add context or child-process variables

`hookSpecificOutput.additional_context` adds text to the Agent context. An
`env` map is injected into the Hook child only:

```toml
[[hooks.SessionStart]]
name = "load-context"
command = "/usr/local/bin/load-context"
env = { TEAM = "platform" }
```

The host process is not changed. Environment names must match
`[A-Za-z_][A-Za-z0-9_]*`; values are not printed. Extension Hooks use the same
config and protocol; see [Extensions](extensions.md).
