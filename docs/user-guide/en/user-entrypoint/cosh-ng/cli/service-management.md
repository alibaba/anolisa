# Service Management

The `cosh-cli svc` subsystem manages system services via systemd. All Linux distributions use the unified `systemctl` interface. macOS does not support this subsystem.

## Command List

| Command | Description |
|---------|-------------|
| `cosh-cli svc status <name>` | View service status |
| `cosh-cli svc start <name>` | Start service |
| `cosh-cli svc stop <name>` | Stop service |
| `cosh-cli svc restart <name>` | Restart service |
| `cosh-cli svc enable <name>` | Enable auto-start on boot |
| `cosh-cli svc disable <name>` | Disable auto-start on boot |
| `cosh-cli svc list` | List services |

## status

View detailed service status including running state, PID, memory usage, and recent logs.

```bash
cosh-cli svc status nginx
```

Output:

```json
{
  "ok": true,
  "data": {
    "name": "nginx",
    "active": true,
    "enabled": true,
    "state": "Running",
    "pid": 1234,
    "memory": "12.5M",
    "uptime_seconds": 86400,
    "recent_logs": [
      "2026-06-30 10:00:01 [notice] worker process started",
      "2026-06-30 10:00:01 [notice] signal process started"
    ]
  },
  "meta": { "subsystem": "svc", "duration_ms": 50, "distro": "alinux", "dry_run": false }
}
```

## start / stop / restart

Control service running state. Supports `--dry-run` preview.

```bash
cosh-cli svc restart nginx
cosh-cli svc restart nginx --dry-run
```

Success output:

```json
{
  "ok": true,
  "data": {
    "name": "nginx",
    "action": "restart",
    "success": true
  },
  "meta": { "subsystem": "svc", "duration_ms": 1200, "distro": "ubuntu", "dry_run": false }
}
```

## enable / disable

Control service auto-start on boot.

```bash
cosh-cli svc enable nginx
cosh-cli svc disable nginx
```

## list

List system services with optional state filtering.

```bash
cosh-cli svc list
cosh-cli svc list --state running
cosh-cli svc list --state failed
```

Output:

```json
{
  "ok": true,
  "data": {
    "services": [
      { "name": "nginx", "active": true, "enabled": true, "state": "Running" },
      { "name": "sshd", "active": true, "enabled": true, "state": "Running" }
    ],
    "total": 2
  },
  "meta": { "subsystem": "svc", "duration_ms": 200, "distro": "centos", "dry_run": false }
}
```

## Service State Enum

| State | Description |
|-------|-------------|
| `Running` | Currently running |
| `Stopped` | Stopped |
| `Failed` | Execution failed |
| `Activating` | Starting up |
| `Deactivating` | Shutting down |
| `Unknown` | State unknown |

## Error Handling

- Service not found returns `SvcNotFound`
- Start failure returns `SvcStartFailed`, stop failure returns `SvcStopFailed`
- Root privileges required returns `PermissionDenied`
