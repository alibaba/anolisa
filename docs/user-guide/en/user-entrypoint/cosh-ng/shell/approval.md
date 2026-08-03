# Tool Approval

[中文版](../../../../zh/user-entrypoint/cosh-ng/shell/approval.md)

cosh shows approval cards before an Agent performs guarded work. The card tells
you which tool will run, what it will receive, and which risks or Hook findings
need attention.

## Choose an approval mode

Switch with `/mode approval <mode>` or set `shell.approval_mode`.

| Mode | Meaning | Mapping to cosh-core |
|------|---------|---------------------|
| `recommend` | Read-only work can run; calls that change state or cross an external boundary ask | `strict` |
| `auto` | Default; eligible low-risk and file-edit tools run, while shell, network, MCP, and external tools ask | `auto` |
| `trust` | Routine calls run without cards after explicit confirmation | `trust` |

Switching to trust mode requires secondary confirmation:

```
/mode approval trust confirm
```

Trust is not a blanket bypass. Irrecoverable system-control commands such as
reboot, shutdown, and halt still require an explicit card, including wrapped
or pre-trusted forms. High-risk cards cannot create a persistent trust key.

## Read an approval card

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

Check the tool name, input preview, risk, and Hook warnings before choosing
Approve or Deny. Use Details when the preview is shortened. If several requests
are waiting, the counter in the upper-right corner shows the queue position.

## Run approved Shell commands

After you approve a `shell` tool, cosh sends the command to the foreground
Shell.

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

The command behaves like one you typed yourself.

- Command output is displayed directly in the terminal
- User can interact in real-time (e.g., confirmation prompts)
- Ctrl+C can interrupt execution

Approved handoffs run one at a time. When a command waits for terminal input,
cosh can show a prompt-tail hint and interrupts eligible password, pager, or
plain-stdin waits after `shell.input_wait_timeout_secs` (120 seconds by
default). Set the value to `0` to disable the timeout. Fullscreen TUIs and
pipeline reads are exempt.

## Review earlier decisions

cosh records approval decisions in its runtime journal. When audit logging is
enabled, a redacted copy is also written to the durable audit timeline. Use the
[audit guide](../cli/audit.md) when you need to investigate an earlier action.

## Configuration

```toml
[shell]
# Approval mode: recommend | auto | trust
approval_mode = "auto"

# Exact command trust keys
trusted_commands = ["ls", "cat", "echo"]

# Input-wait timeout for approved foreground commands; 0 disables it
input_wait_timeout_secs = 120
```

`trusted_commands` uses exact trust keys, not arbitrary shell substrings, and
never overrides the irrecoverable-command gate. See
[Configuration](../configuration.md) for environment overrides.
