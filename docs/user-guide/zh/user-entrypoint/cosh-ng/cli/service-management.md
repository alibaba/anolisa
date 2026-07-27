# 服务管理

`cosh-cli svc` 子系统通过 systemd 管理系统服务。所有 Linux 发行版使用统一的
`systemctl` 接口，macOS 不支持此子系统。

## 命令列表

| 命令 | 说明 |
|------|------|
| `cosh-cli svc status <name>` | 查看服务状态 |
| `cosh-cli svc start <name>` | 启动服务 |
| `cosh-cli svc stop <name>` | 停止服务 |
| `cosh-cli svc restart <name>` | 重启服务 |
| `cosh-cli svc enable <name>` | 启用开机自启 |
| `cosh-cli svc disable <name>` | 禁用开机自启 |
| `cosh-cli svc list` | 列出服务 |

## status

查看服务的详细状态，包括运行状态、PID、内存占用和最近日志。

```bash
cosh-cli svc status nginx
```

输出：

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

控制服务运行状态。支持 `--dry-run` 预览。

```bash
cosh-cli svc restart nginx
cosh-cli svc restart nginx --dry-run
```

成功输出：

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

控制服务的开机自启状态。

```bash
cosh-cli svc enable nginx
cosh-cli svc disable nginx
```

## list

列出系统服务，支持按状态过滤。

```bash
cosh-cli svc list
cosh-cli svc list --state running
cosh-cli svc list --state failed
```

输出：

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

## 服务状态枚举

| 状态 | 说明 |
|------|------|
| `Running` | 正在运行 |
| `Stopped` | 已停止 |
| `Failed` | 运行失败 |
| `Activating` | 正在启动 |
| `Deactivating` | 正在停止 |
| `Unknown` | 状态未知 |

## 错误处理

- 服务不存在返回 `SvcNotFound`
- 启动失败返回 `SvcStartFailed`，停止失败返回 `SvcStopFailed`
- 需要 root 权限时返回 `PermissionDenied`
