# cosh-cli 总览

cosh-cli 是 cosh-ng 的结构化命令行工具，为 AI Agent 提供零学习成本的跨发行版
系统操作接口。所有命令输出 JSON 格式的 `CoshResponse<T>` 信封。

## 设计理念

1. **零学习** — Agent 无需区分 dnf / apt / zypper
2. **结构化输出** — 纯 JSON，无需正则解析文本
3. **可逆** — checkpoint 创建 → 执行 → 失败时回滚
4. **分类错误** — `recoverable` 字段告知 Agent 是否值得重试
5. **Dry-run** — 所有写操作支持 `--dry-run` 预览

## 命令子系统

| 子系统 | 说明 | 详细文档 |
|--------|------|----------|
| `pkg` | 跨发行版包管理 | [package-management.md](package-management.md) |
| `svc` | systemd 服务管理 | [service-management.md](service-management.md) |
| `checkpoint` | 工作区快照（ws-ckpt） | [checkpoint.md](checkpoint.md) |
| `audit` | 安全策略审计 | [audit.md](audit.md) |

## 通用选项

```
cosh-cli <SUBCOMMAND> [OPTIONS]
```

| 选项 | 说明 |
|------|------|
| `--help` / `-h` | 显示帮助信息 |
| `--version` / `-V` | 显示版本号 |
| `--dry-run` | 预览模式，不实际执行（各写操作子命令各自支持） |

## 快速示例

```bash
# 包管理
cosh-cli pkg install nginx
cosh-cli pkg search "web server"
cosh-cli pkg list --installed
cosh-cli pkg remove nginx --dry-run

# 服务管理
cosh-cli svc status nginx
cosh-cli svc restart nginx --dry-run
cosh-cli svc list --state running

# 工作区快照
cosh-cli checkpoint create --workspace /home/agent/project --id step-042 -m "before refactor"
cosh-cli checkpoint restore step-040 --workspace /home/agent/project
cosh-cli checkpoint list --workspace /home/agent/project

# 安全审计
cosh-cli audit check --action "rm -rf /var/log"
cosh-cli audit log --session abc123
cosh-cli audit policy show
```
