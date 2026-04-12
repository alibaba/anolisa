# Copilot-Shell Hooks

Hooks are scripts or programs that copilot-shell executes at specific points in
the agentic loop, allowing you to intercept and customize behavior without
modifying the CLI's source code.

## What are hooks?

Hooks run synchronously as part of the agent loop — when a hook event fires,
copilot-shell waits for all matching hooks to complete before continuing.

With hooks, you can:

- **Add context:** Inject relevant information (like git history) before the
  model processes a request.
- **Validate actions:** Review tool arguments and block potentially dangerous
  operations.
- **Enforce policies:** Implement security scanners and compliance checks.
- **Log interactions:** Track tool usage and model responses for auditing.
- **Optimize behavior:** Dynamically filter available tools or adjust model
  parameters.
- **Sandbox commands:** Automatically wrap dangerous shell commands in a
  linux-sandbox for isolated execution.

### Getting started

- **[Writing hooks guide](writing-hooks.md)**: A tutorial on creating your first
  hook with comprehensive examples.
- **[Hooks reference](reference.md)**: The definitive technical specification of
  I/O schemas and exit codes.

## Core concepts

### Hook events

Hooks are triggered by specific events in copilot-shell's lifecycle.
The table below reflects the currently wired events in the runtime hook system.

| Event                 | When It Fires                                  | Impact          | Common Use Cases                             |
| --------------------- | ---------------------------------------------- | --------------- | -------------------------------------------- |
| `SessionStart`        | When a session begins (startup, resume, clear) | Inject Context  | Initialize resources, load context           |
| `SessionEnd`          | When a session ends (exit, clear)              | Advisory        | Clean up, save state                         |
| `UserPromptSubmit`    | After user submits prompt, before planning     | Block / Context | Add context, validate prompts, block turns   |
| `Stop`                | When agent is about to stop                    | Retry / Halt    | Review output, force retry or halt execution |
| `BeforeModel`         | Before sending request to LLM                  | Block / Mock    | Modify prompts, swap models, mock responses  |
| `AfterModel`          | After receiving LLM response                   | Block / Observe | Filter responses, log interactions           |
| `BeforeToolSelection` | Before LLM selects tools                       | Filter Tools    | Filter available tools, optimize selection   |
| `PreToolUse`          | Before a tool executes                         | Block / Rewrite | Validate arguments, block dangerous ops      |
| `PostToolUse`         | After a tool executes                          | Block / Context | Process results, run tests, hide results     |
| `PostToolUseFailure`  | After a tool execution fails                   | Recovery        | Extract original command, sandbox bypass     |
| `PreCompact`          | Before context compression                     | Advisory        | Save state, notify user                      |
| `Notification`        | When a system notification occurs              | Advisory        | Forward to desktop alerts, logging           |
| `PermissionRequest`   | When a permission dialog is displayed          | Allow / Deny    | Auto-approve or auto-deny tool permissions   |

### Global mechanics

#### Strict JSON requirements (The "Golden Rule")

Hooks communicate via `stdin` (Input) and `stdout` (Output).

1. **Silence is Mandatory**: Your script **must not** print any plain text to
   `stdout` other than the final JSON object.
2. **Pollution = Failure**: If `stdout` contains non-JSON text, parsing will
   fail. The CLI will default to "Allow" and treat the entire output as a
   `systemMessage`.
3. **Debug via Stderr**: Use `stderr` for **all** logging and debugging (e.g.,
   `echo "debug" >&2`). copilot-shell captures `stderr` but never attempts to
   parse it as JSON.

#### Exit codes

| Exit Code | Label            | Behavioral Impact                                                                |
| --------- | ---------------- | -------------------------------------------------------------------------------- |
| **0**     | **Success**      | `stdout` is parsed as JSON. **Preferred code** for all logic.                    |
| **2**     | **System Block** | Critical block. The action is aborted. `stderr` is used as the rejection reason. |
| **Other** | **Warning**      | Non-fatal failure. A warning is shown; the interaction proceeds.                 |

#### Matchers

You can filter which specific tools or triggers fire your hook using the
`matcher` field.

- **Tool events** (`PreToolUse`, `PostToolUse`): Matchers are **Regular
  Expressions** (e.g., `"write_.*"`).
- **Lifecycle events**: Matchers are **Exact Strings** (e.g., `"startup"`).
- **Wildcards**: `"*"` or `""` (empty string) matches all occurrences.

#### When Multiple Hooks Match

If multiple hooks match the same event in one turn, copilot-shell applies the
following execution and merge rules:

1. **Plan and deduplicate**:
   - Hooks are selected by event + matcher.
   - Duplicate hooks are removed using a stable key (`name:command` or
     `command` if `name` is empty).

2. **Execution mode**:
   - By default, matching hooks run in **parallel**.
   - If **any** matched hook definition has `sequential: true`, all matched
     hooks for that event run **sequentially**.

3. **Sequential chaining (event-specific)**:
   - `PreToolUse`: a hook can modify `tool_input`; later hooks see the modified
     input.
   - `UserPromptSubmit`: a hook can append `additionalContext`; later hooks see
     the updated prompt.

4. **Final output merge**:
   - `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `Stop`:
     restrictive outcomes win (`deny`/`block`), reasons are concatenated,
     `continue=false` takes precedence.
   - `BeforeModel`, `AfterModel`: later hook fields overwrite earlier fields.
   - `BeforeToolSelection`: `allowedFunctionNames` are unioned;
     mode priority is `NONE > ANY > AUTO`.
   - `PermissionRequest`: deny wins over allow; messages are concatenated;
     `interrupt=true` takes precedence.

For full field-level details, see [Hooks reference](reference.md).

## Configuration

Hooks are configured in `settings.json`. copilot-shell merges configurations
from multiple layers in the following order of precedence (highest to lowest):

1. **Project settings**: `.copilot-shell/settings.json` in the current directory.
2. **User settings**: `~/.copilot-shell/settings.json`.
3. **System settings**: `/etc/copilot-shell/settings.json`.
4. **Extensions**: Hooks defined by installed extensions.

### Example configuration

```json
{
  "hooks": {
    "enabled": true,
    "PreToolUse": [
      {
        "matcher": "run_shell_command",
        "sequential": true,
        "hooks": [
          {
            "type": "command",
            "command": "python3 hooks/sandbox-guard.py",
            "name": "sandbox-guard",
            "timeout": 10000,
            "description": "Wraps dangerous commands in linux-sandbox"
          }
        ]
      }
    ],
    "BeforeModel": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "node hooks/model-router.js",
            "name": "model-router",
            "timeout": 5000,
            "description": "Routes requests to different models based on complexity"
          }
        ]
      }
    ]
  }
}
```

### Hook configuration fields

| Field         | Type     | Required | Description                                                        |
| ------------- | -------- | -------- | ------------------------------------------------------------------ |
| `type`        | `string` | **Yes**  | The execution engine. Currently only `"command"` is supported.     |
| `command`     | `string` | **Yes**  | The shell command to execute.                                      |
| `name`        | `string` | No       | A friendly name for identifying the hook in logs and CLI commands. |
| `timeout`     | `number` | No       | Execution timeout in milliseconds (default: 60000).                |
| `description` | `string` | No       | A brief explanation of the hook's purpose.                         |

### Environment variables

Hooks are executed with a sanitized environment:

- `COPILOT_SHELL_PROJECT_DIR`: The absolute path to the project root. Also available as
  `GEMINI_PROJECT_DIR`, `CLAUDE_PROJECT_DIR`, and `QWEN_PROJECT_DIR` for compatibility.

## Security and risks

> **WARNING**: Hooks execute arbitrary code with your user privileges. By
> configuring hooks, you are allowing scripts to run shell commands on your
> machine.

**Project-level hooks** are particularly risky when opening untrusted projects.
copilot-shell **fingerprints** project hooks. If a hook's name or command
changes (e.g., via `git pull`), it is treated as a **new, untrusted hook** and
you will be warned before it executes.

See [Hooks reference](reference.md) for the full threat model and security
considerations.

## Managing hooks

Use the CLI commands to manage hooks without editing JSON manually:

- **View hooks:** `/hooks panel`
- **Enable/Disable all:** `/hooks enable-all` or `/hooks disable-all`
- **Toggle individual:** `/hooks enable <name>` or `/hooks disable <name>`

## Built-in hooks

copilot-shell ships with two built-in hook scripts:

### `sandbox-guard.py` (PreToolUse)

Automatically detects dangerous shell commands and wraps them in `linux-sandbox`
for isolated execution. Supports:

- **Block patterns**: Commands that are outright blocked (shutdown, reboot, fork
  bomb).
- **Sandbox patterns**: Dangerous filesystem operations wrapped in sandbox (rm
  -rf, chmod, chown).
- **Network patterns**: Network commands that need sandbox with network access
  enabled.
- **Sudo handling**: Automatic sudo prefix stripping for sandbox execution.

### `sandbox-failure-handler.py` (PostToolUseFailure)

Handles sandbox execution failures by extracting the original command and
emitting a `sandbox_bypass_request` for user approval.
