# 管理系统操作

[English](../../../../en/user-entrypoint/cosh-ng/cli/overview.md)

脚本或 Agent 需要稳定的系统操作 JSON 接口时，可以使用 `cosh-cli`。它会识别当前
平台，调用对应后端，并在统一响应中返回错误类型、重试建议、耗时、平台和预览状态。

## 命令域

| 命令域 | 操作 | 后端 |
|---|---|---|
| `pkg` | `install`、`remove`、`search`、`list` | dnf、apt、zypper 或 Homebrew |
| `svc` | `status`、`start`、`stop`、`restart`、`enable`、`disable`、`list` | systemd |
| `checkpoint` | `init`、`recover`、`create`、`list`、`restore`、`status`、`delete`、`diff`、`cleanup` | ws-ckpt Unix socket |
| `audit` | `check`、`log`、`status`、`events`、`trace`、`export`、`prune`、`policy` | 审计策略和存储 |

运行 `cosh-cli <domain> --help` 和 `cosh-cli <domain> <action> --help` 查看精确参数。

## 安全的初次示例

```bash
# Read-only
cosh-cli pkg search "web server"
cosh-cli pkg list --installed
cosh-cli svc status nginx
cosh-cli svc list --state running
cosh-cli audit status

# 预览支持 dry-run 的软件包/服务修改
cosh-cli pkg install nginx --dry-run
cosh-cli svc restart nginx --dry-run
```

`--dry-run` 只适用于部分操作。软件包安装、卸载和服务状态修改支持该参数，快照操作
不支持。执行快照命令前，请仔细检查工作区和快照参数。

Linux 上的软件包和服务修改通常需要 root 权限。服务操作依赖 systemd，不能在 macOS
上使用。快照命令需要 ws-ckpt 守护进程正在运行，指定的工作区路径也必须存在。

## 在脚本或 Agent 中处理响应

1. 把 stdout 解析为一个 `CoshResponse<T>` JSON 值。
2. 检查 `ok`，不要根据说明文字推断是否成功。
3. 失败时使用 `error.recoverable` 判断重试是否有意义，并根据 `error.hint` 决定下一步。
4. 判断修改是否已经发生前，先确认 `meta.dry_run`。
5. 单独保留 stderr，stdout 是稳定的自动化接口。

继续阅读[输出格式](../output-format.md)、[软件包管理](package-management.md)、
[服务管理](service-management.md)、[Checkpoints](checkpoint.md)和[Audit](audit.md)。
