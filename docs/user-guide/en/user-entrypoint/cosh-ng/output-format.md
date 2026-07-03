# Output Format

All cosh-cli commands output a unified JSON envelope `CoshResponse<T>`, making it easy for AI Agents to parse.

## Success Response

```json
{
  "ok": true,
  "data": { ... },
  "meta": {
    "subsystem": "pkg",
    "duration_ms": 342,
    "distro": "alinux",
    "dry_run": false
  }
}
```

## Error Response

```json
{
  "ok": false,
  "error": {
    "code": "PkgNotFound",
    "message": "package 'nginx-extra' not found",
    "recoverable": true,
    "hint": "try 'cosh-cli pkg search nginx'",
    "subsystem": "pkg"
  },
  "meta": {
    "subsystem": "pkg",
    "duration_ms": 120,
    "distro": "ubuntu",
    "dry_run": false
  }
}
```

## Key Fields

| Field | Type | Description |
|-------|------|-------------|
| `ok` | bool | Whether the operation succeeded |
| `data` | object | Business data carried on success |
| `error.code` | string | Error code enum (see below) |
| `error.recoverable` | bool | Whether the Agent should retry |
| `error.hint` | string | Suggested next action |
| `meta.subsystem` | string | Source subsystem (pkg/svc/checkpoint/audit) |
| `meta.duration_ms` | u64 | Operation duration (milliseconds) |
| `meta.distro` | string | Detected distribution |
| `meta.dry_run` | bool | Whether this was a preview |

## Error Codes

| Error Code | Subsystem | Meaning |
|-----------|-----------|---------|
| `PkgNotFound` | pkg | Package does not exist |
| `PkgBackendError` | pkg | Package manager execution failed |
| `UnsupportedDistro` | pkg/svc | Unsupported distribution |
| `SvcNotFound` | svc | Service does not exist |
| `SvcStartFailed` | svc | Service start failed |
| `SvcStopFailed` | svc | Service stop failed |
| `CheckpointDaemonUnavailable` | checkpoint | ws-ckpt daemon not running |
| `CheckpointNotFound` | checkpoint | Snapshot does not exist |
| `AuditDenied` | audit | Policy denied |
| `Timeout` | * | Command execution timed out |
| `PermissionDenied` | * | Insufficient permissions |

## Exit Codes

- `0` — Operation successful (`ok: true`)
- `1` — Operation failed (`ok: false`)

## Agent Integration Pattern

For AI Agents consuming cosh-cli output, the recommended pattern is:

1. Parse JSON, check the `ok` field
2. If `ok: false`, read `error.recoverable` to decide whether to retry
3. If not recoverable, display `error.hint` to the user or execute the suggested command
4. When `meta.dry_run` is `true`, this is a preview result and the actual operation has not been executed
