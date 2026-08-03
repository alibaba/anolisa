# 安全审计与生产排障

[English](../../../../en/user-entrypoint/cosh-ng/cli/audit.md)

`cosh-cli audit` 可以评估安全策略，也能生成经过脱敏和关联的事件时间线，供生产排障
使用。
`cosh-core`、`cosh-shell` 和策略检查会追加版本化 JSONL 事件，既有 SLS/metrics
导出保持不变。

先通过 ANOLISA CLI 安装 cosh-ng，再从本机或事故处理流程调用这些命令。

```bash
sudo anolisa --install-mode system install cosh-ng
```

所有命令均返回标准的 `CoshResponse<T>` JSON 响应。

## 运维命令

| 命令 | 用途 |
| --- | --- |
| `cosh-cli audit status` | 查看生效配置、存储、读取诊断和最后观测到的健康状态 |
| `cosh-cli audit events` | 按过滤条件查询有界事件页和不透明游标 |
| `cosh-cli audit trace <id>` | 关联事件、会话、任务、轮次、请求、工具调用或命令 ID |
| `cosh-cli audit export --output <dir>` | 生成脱敏事故包，脱敏失败时停止导出 |
| `cosh-cli audit prune --dry-run` | 预览确定的保留计划，版本 1 不提供手工删除 |

```bash
cosh-cli audit status
cosh-cli audit events --since 2h --event approval.requested,approval.resolved --limit 100
cosh-cli audit trace 7fa4c0b0-0000-4000-8000-000000000001
cosh-cli audit export --since 2h --identity session-123 --output ./audit-incident
cosh-cli audit prune --dry-run
```

`events` 支持 `--since`、`--until`，可重复或逗号分隔的 `--event`、`--component`、
`--outcome`，以及 `--identity`、`--schema v1|legacy_v0`、`--limit 1..1000` 和
`--cursor`。`--since` 可使用 `30s`、`5m`、`2h`、`1d`，绝对时间使用 RFC 3339。
游标绑定原始过滤条件，换过滤条件后会被拒绝。

导出目录包含 `events.jsonl`、`summary.json`、`manifest.json` 和 `SHA256SUMS`。
导出过程会替换关联身份、只保留允许字段、扫描最终内容中的秘密，并以原子方式发布目录。
`--force` 只能替换带有有效 cosh 审计清单的目录。

在 `cosh-shell` 内可使用 `/audit status`、`/audit trace current` 和
`/audit export current <dir>`，它们是同一 CLI 的有界入口。

## 配置与存储

不会新增审计专用配置文件。`/etc/copilot-shell/config.toml` 中的系统 `[audit]`
表优先级最高。系统文件没有该表时，才使用 `~/.copilot-shell/config.toml`。
项目 `[audit]` 表会被忽略，工作区不能削弱生产审计。

```toml
[audit]
mode = "best_effort" # best_effort | required
retention_days = 30
max_disk_bytes = 1073741824
```

存储根目录按以下顺序解析。

1. `COSH_AUDIT_DIR`（部署/测试覆盖）
2. `$XDG_STATE_HOME/cosh/audit`
3. `~/.local/state/cosh/audit`

不会回退到临时目录。目录权限为 `0700`，分段和状态文件为 `0600`。每个写入者在
`v1/segments/YYYY-MM-DD/` 下创建独立加锁文件，达到 16 MiB 或跨 UTC 日期时轮转，
关闭后通过原子重命名发布 `.jsonl`。`v1/state.json` 只是最后观测者诊断状态，不能作为
授权依据。

保留任务最多每 24 小时运行一次，先删除超期的已关闭 segment，再应用磁盘上限，绝不
删除仍被活跃写入者加锁的文件。默认保留 30 天、上限 1 GiB。

## 失败模式

- `best_effort` 显示简短的降级警告并允许工作继续。
- `required` 在模型服务启动、审批决议或工具执行前无法持久写入边界记录时停止操作。
- 审计不可用时原生 PTY 命令仍可使用，但会显示持续的审计缺口。
- 查询会简要报告损坏记录，崩溃留下的不完整尾部记录不会作为事件返回。

不要把私有分段目录直接附到工单。应使用 `audit export`，复核清单和哈希，
再通过批准的事故通道传输脱敏包。

## 策略评估兼容性

原有策略命令继续可用。

```bash
cosh-cli audit check --action "rm -rf /var/log"
cosh-cli audit log --session abc123
cosh-cli audit policy show
cosh-cli audit policy list
cosh-cli audit policy validate ./audit.toml
cosh-cli audit policy explain "cat /etc/os-release"
```

策略加载顺序仍为 `COSH_AUDIT_POLICY`、`~/.copilot-shell/cosh/audit.toml`、
`/etc/cosh/audit.toml`、内置 `balanced`。旧策略日志可通过 `legacy_v0` 读取；原始旧版
内容不会投影到版本 1 查询或导出。
