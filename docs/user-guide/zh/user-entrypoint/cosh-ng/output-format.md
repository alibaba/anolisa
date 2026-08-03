# 输出格式

[English](../../../en/user-entrypoint/cosh-ng/output-format.md)

cosh-cli 的所有命令输出统一的 JSON 信封 `CoshResponse<T>`，方便 AI Agent 解析。

## 成功响应

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

## 错误响应

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

## 关键字段

| 字段 | 类型 | 说明 |
|------|------|------|
| `ok` | bool | 操作是否成功 |
| `data` | object | 成功时携带的业务数据 |
| `error.code` | string | 错误码枚举（详见下方） |
| `error.recoverable` | bool | Agent 是否值得重试 |
| `error.hint` | string | 建议的下一步操作 |
| `meta.subsystem` | string | 来源子系统（pkg/svc/checkpoint/audit） |
| `meta.duration_ms` | u64 | 操作耗时（毫秒） |
| `meta.distro` | string | 当前检测到的发行版 |
| `meta.dry_run` | bool | 是否为预览模式 |

## 错误码

| 错误码 | 子系统 | 含义 |
|--------|--------|------|
| `PkgNotFound` | pkg | 包不存在 |
| `PkgBackendError` | pkg | 包管理器执行失败 |
| `UnsupportedDistro` | pkg/svc | 不支持的发行版 |
| `SvcNotFound` | svc | 服务不存在 |
| `SvcStartFailed` | svc | 服务启动失败 |
| `SvcStopFailed` | svc | 服务停止失败 |
| `CheckpointDaemonUnavailable` | checkpoint | ws-ckpt 守护进程未运行 |
| `CheckpointNotFound` | checkpoint | 快照不存在 |
| `AuditDenied` | audit | 策略拒绝 |
| `Timeout` | * | 命令执行超时 |
| `PermissionDenied` | * | 权限不足 |

## 退出码

- `0` 表示操作成功，响应中的 `ok` 为 `true`
- `1` 表示操作失败，响应中的 `ok` 为 `false`

## Agent 对接模式

AI Agent 处理 cosh-cli 输出时，可以按以下步骤执行。

1. 解析 JSON，检查 `ok` 字段
2. 若 `ok: false`，读取 `error.recoverable` 决定是否重试
3. 若不可恢复，展示 `error.hint` 给用户或执行 hint 中的建议命令
4. `meta.dry_run` 为 `true` 时，表示这是预览结果，实际操作尚未执行
