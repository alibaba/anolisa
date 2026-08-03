# Manage System Operations

[中文版](../../../../zh/user-entrypoint/cosh-ng/cli/overview.md)

Use `cosh-cli` when a script or Agent needs a stable JSON interface to supported
system operations. It normalizes platform-specific backends and reports typed
errors, retry guidance, duration, platform, and preview state in one envelope.

## Command domains

| Domain | Actions | Backend |
|---|---|---|
| `pkg` | `install`, `remove`, `search`, `list` | dnf, apt, zypper, or Homebrew |
| `svc` | `status`, `start`, `stop`, `restart`, `enable`, `disable`, `list` | systemd |
| `checkpoint` | `init`, `recover`, `create`, `list`, `restore`, `status`, `delete`, `diff`, `cleanup` | ws-ckpt Unix socket |
| `audit` | `check`, `log`, `status`, `events`, `trace`, `export`, `prune`, `policy` | audit policy and store |

Run `cosh-cli <domain> --help` and
`cosh-cli <domain> <action> --help` for exact flags.

## Safe first examples

```bash
# Read-only
cosh-cli pkg search "web server"
cosh-cli pkg list --installed
cosh-cli svc status nginx
cosh-cli svc list --state running
cosh-cli audit status

# Preview supported package/service mutations
cosh-cli pkg install nginx --dry-run
cosh-cli svc restart nginx --dry-run
```

`--dry-run` is action-specific, not a global flag. Package install/remove and
service state mutations support it. Checkpoint mutations do not; review their
workspace and snapshot arguments carefully.

Linux package and service mutations normally require root privileges. Service
operations require systemd and are unavailable on macOS. Checkpoint commands
require a running ws-ckpt daemon and an existing workspace path.

## Agent consumption pattern

1. Parse stdout as one `CoshResponse<T>` JSON value.
2. Check `ok`; do not infer success from human-readable text.
3. On failure, use `error.recoverable` to decide whether retry makes sense and
   `error.hint` for the next action.
4. Confirm `meta.dry_run` before assuming a mutation occurred.
5. Preserve stderr separately; stdout is the automation contract.

See [Output format](../output-format.md), [Package management](package-management.md),
[Service management](service-management.md), [Checkpoints](checkpoint.md), and
[Audit](audit.md).
