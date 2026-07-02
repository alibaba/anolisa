# Security Audit

The `cosh-cli audit` subsystem implements a PEP/PDP architecture for security policy evaluation. Before executing dangerous operations, Agents can first check whether an operation is allowed through the audit subsystem.

## Core Concepts

- **PEP** (Policy Enforcement Point) — Executes the complete evaluation + logging flow
- **PDP** (Policy Decision Point) — Pure decision logic, returns decisions based on policy rules
- **Decision** — Evaluation result: Allow / Deny / RequireApproval
- **Policy** — Policy rule set, divided into built-in presets and custom policies

## Command List

| Command | Description |
|---------|-------------|
| `cosh-cli audit check --action <cmd>` | Evaluate command safety |
| `cosh-cli audit log` | View audit log |
| `cosh-cli audit policy show` | Display current policy |

## check

Evaluates whether a shell command is allowed by policy.

```bash
cosh-cli audit check --action "rm -rf /var/log"
```

Output:

```json
{
  "ok": true,
  "data": {
    "outcome": "Deny",
    "matched_rule": "shell-deny-destructive",
    "reason": "destructive command matches deny pattern"
  },
  "meta": { "subsystem": "audit", "duration_ms": 2, "distro": "alinux", "dry_run": false }
}
```

Safe command example:

```bash
cosh-cli audit check --action "cat /etc/os-release"
```

```json
{
  "ok": true,
  "data": {
    "outcome": "Allow",
    "matched_rule": null,
    "reason": "no deny rules matched"
  },
  "meta": { "subsystem": "audit", "duration_ms": 1, "distro": "alinux", "dry_run": false }
}
```

## log

View audit log entries.

```bash
cosh-cli audit log --session abc123
```

Audit logs are written to the path specified by `$COSH_AUDIT_LOG`. If not set, defaults to `~/.copilot-shell/audit.log`.

## policy show

Display the currently effective audit policy.

```bash
cosh-cli audit policy show
```

## Built-in Policy Presets

| Preset | Description |
|--------|-------------|
| `permissive` | Permissive mode, most operations Allow |
| `balanced` | Balanced mode (default), write operations require approval |
| `strict` | Strict mode, almost all non-read-only operations Deny |

## Policy Loading Priority

1. Policy file specified by `COSH_AUDIT_POLICY` environment variable
2. `~/.copilot-shell/cosh/audit.toml` (user-level)
3. `/etc/cosh/audit.toml` (system-level)
4. Built-in `balanced` preset

## Action Parsing

`parse_action_string()` parses raw shell strings into structured `Action`:

- Tokenizes by whitespace (spaces, tabs, newlines)
- Detects shell metacharacters (`;` `|` `&` `>` `<` `$` `` ` `` `(` `)` `{` `}`)
- Commands containing metacharacters are directly rejected (cannot be safely analyzed)

## Log Redaction

Audit logs are automatically redacted for sensitive fields before writing:

- Password arguments (`--password`, `-p` followed values)
- API keys and tokens
- Secret values in environment variables

## Decision Enum

| Decision | Meaning | Agent Behavior |
|----------|---------|---------------|
| `Allow` | Policy allows | Execute directly |
| `Deny` | Policy denies | Abort, do not execute |
| `RequireApproval` | Requires human confirmation | Pause, wait for user approval |
