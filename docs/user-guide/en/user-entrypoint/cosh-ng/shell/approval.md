# Tool Approval

cosh-shell's tool approval system presents operation details as visual cards when an AI adapter requests tool execution, letting the user decide whether to allow it.

## Approval Modes

Switch via `/mode approval <mode>` or configure `shell.approval_mode`:

| Mode | Meaning | Mapping to cosh-core |
|------|---------|---------------------|
| `recommend` | Recommend mode: all tool calls require approval | `strict` |
| `auto` | Auto mode (default): only shell commands require approval | `auto` |
| `trust` | Trust mode: all tools auto-execute (requires `confirm`) | `trust` |

Switching to trust mode requires secondary confirmation:

```
/mode approval trust confirm
```

## Approval Cards

When a tool requires approval, cosh-shell renders an inline approval panel:

```
┌─────────────────────────────────────────┐
│ 🔧 Tool: shell                    [1/3] │
│ Risk: medium                            │
│─────────────────────────────────────────│
│ Command:                                │
│   rm -rf /tmp/old-build                 │
│─────────────────────────────────────────│
│ ⚠ Hook: sandbox-guard                   │
│   "Command matches risk pattern"         │
│─────────────────────────────────────────│
│ [✓ Approve]  [ Deny ]  [ Details ]      │
└─────────────────────────────────────────┘
```

### Card Elements

| Element | Description |
|---------|-------------|
| Tool | Tool name |
| Risk | Risk level (assessed by hooks) |
| Queue | Queue position (when multiple requests are queued) |
| Command/Input | Tool input preview |
| Hook warnings | Warning messages from hooks |
| Actions | Available action buttons |

### User Actions

| Action | Description |
|--------|-------------|
| Approve | Allow execution |
| Deny | Reject execution |
| Details | Expand full input content |

## Shell Command Handoff

When the approved tool is of `shell` type and the user approves, the command is "handed off" to the foreground PTY for execution (rather than being executed by cosh-core in the background):

```
User approves shell command
       │
       ▼
cosh-shell injects command into PTY
       │
       ▼
bash/zsh executes in foreground (user can interact)
       │
       ▼
Execution result returned via OSC markers
```

This means:
- Command output is displayed directly in the terminal
- User can interact in real-time (e.g., confirmation prompts)
- Ctrl+C can interrupt execution

## Approval Journal

All approval decisions are recorded in an in-memory journal:

| Field | Description |
|-------|-------------|
| `id` | Approval request unique identifier |
| `run_id` | Associated Agent run ID |
| `kind` | Request type (Tool / ShellCommand) |
| `risk` | Risk level |
| `decision` | Final decision (Allow / Deny / Cancel) |
| `subject` | Tool name |
| `preview` | Operation preview |

## Relationship with cosh-core Approval Protocol

cosh-shell's approval system is the frontend implementation of the `can_use_tool` control request in the cosh-core JSONL protocol:

```
Core → Shell:  {"type":"control_request","request_id":"apr-1","request":{"subtype":"can_use_tool",...}}
                       │
                       ▼
              cosh-shell renders approval card
                       │
                       ▼ (user decision)
Shell → Core:  {"type":"control_response","response":{"subtype":"tool_approval","request_id":"apr-1","response":{"behavior":"allow"}}}
```

## Configuration

```toml
[shell]
# Approval mode: recommend | auto | trust
approval_mode = "auto"

# Trusted command list (always auto-approved)
trusted_commands = ["ls", "cat", "echo"]
```
