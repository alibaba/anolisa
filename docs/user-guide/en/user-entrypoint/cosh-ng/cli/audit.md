# Security Audit and Production Troubleshooting

The `cosh-cli audit` subsystem evaluates security policy and provides a redacted, correlated
timeline for production troubleshooting. `cosh-core`, `cosh-shell`, and policy checks append
versioned JSONL events without changing the existing SLS/metrics export.

Install cosh with `anolisa install cosh`, then use these commands locally or from an incident
runbook. Every command returns the standard `CoshResponse<T>` JSON envelope.

## Operational Commands

| Command | Purpose |
| --- | --- |
| `cosh-cli audit status` | Report effective settings, storage, reader diagnostics, and last-observer health |
| `cosh-cli audit events` | Query a bounded event page with filters and an opaque cursor |
| `cosh-cli audit trace <id>` | Correlate an event, session, run, turn, request, Tool-use, or command ID |
| `cosh-cli audit export --output <dir>` | Create a fail-closed redacted incident bundle |
| `cosh-cli audit prune --dry-run` | Preview the deterministic retention plan; version 1 never deletes manually |

Examples:

```bash
cosh-cli audit status
cosh-cli audit events --since 2h --event approval.requested,approval.resolved --limit 100
cosh-cli audit trace 7fa4c0b0-0000-4000-8000-000000000001
cosh-cli audit export --since 2h --identity session-123 --output ./audit-incident
cosh-cli audit prune --dry-run
```

`events` accepts `--since`, `--until`, repeated or comma-separated `--event`, `--component`, and
`--outcome` filters, plus `--identity`, `--schema v1|legacy_v0`, `--limit 1..1000`, and `--cursor`.
Durations such as `30s`, `5m`, `2h`, and `1d` are accepted for `--since`; absolute bounds use RFC
3339. Cursors are bound to the original filters and are rejected if reused with different filters.

The export directory contains `events.jsonl`, `summary.json`, `manifest.json`, and `SHA256SUMS`.
Export aliases correlation identities, applies an allowlist, scans final bytes for secrets, and
publishes atomically. `--force` replaces only a directory containing a valid cosh audit manifest.

Inside `cosh-shell`, `/audit status`, `/audit trace current`, and
`/audit export current <dir>` provide bounded wrappers around the same CLI.

## Configuration and Storage

No separate audit configuration file is added. The existing
`/etc/copilot-shell/config.toml` system table is authoritative; when it has no `[audit]` table,
`~/.copilot-shell/config.toml` is used. Project `[audit]` tables are ignored so a workspace cannot
weaken production audit.

```toml
[audit]
mode = "best_effort" # best_effort | required
retention_days = 30
max_disk_bytes = 1073741824
```

The storage root resolves in this order:

1. `COSH_AUDIT_DIR` (deployment/test override)
2. `$XDG_STATE_HOME/cosh/audit`
3. `~/.local/state/cosh/audit`

There is no temporary-directory fallback. Directories use mode `0700`; segment and state files use
`0600`. Writers create independent locked files below `v1/segments/YYYY-MM-DD/`, rotate at 16 MiB
or a UTC date boundary, and publish closed `.jsonl` segments by atomic rename. `v1/state.json` is
diagnostic last-observer state, not an authorization source.

Retention runs at most once per 24 hours, removes expired closed segments before applying the disk
cap, and never removes a live locked segment. Defaults are 30 days and 1 GiB.

## Failure Modes

- `best_effort` records a bounded degraded warning and lets work continue.
- `required` fails closed before Provider start, approval resolution, or Tool execution when the
  security-boundary record cannot be durably written.
- Native PTY commands remain usable during an audit outage but expose a persistent audit gap.
- Query corruption is reported as bounded diagnostics; a trailing partial crash record is not
  returned as an event.

Never attach the private segment directory directly to a ticket. Use `audit export`, review the
manifest and hashes, and transfer the redacted bundle through the approved incident channel.

## Policy Evaluation Compatibility

The existing policy commands remain available:

```bash
cosh-cli audit check --action "rm -rf /var/log"
cosh-cli audit log --session abc123
cosh-cli audit policy show
cosh-cli audit policy list
cosh-cli audit policy validate ./audit.toml
cosh-cli audit policy explain "cat /etc/os-release"
```

Policy loading remains `COSH_AUDIT_POLICY`, `~/.copilot-shell/cosh/audit.toml`,
`/etc/cosh/audit.toml`, then the built-in `balanced` preset. The legacy policy log is readable as
`legacy_v0`; raw legacy content is not projected into version 1 queries or exports.
