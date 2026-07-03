# 安全审计

`cosh-cli audit` 子系统实现 PEP/PDP 架构的安全策略评估。在执行危险操作前，
Agent 可以先通过 audit 子系统检查操作是否被允许。

## 核心概念

- **PEP**（策略执行点）— 执行完整的评估 + 日志记录流程
- **PDP**（策略决策点）— 纯决策逻辑，根据策略规则返回决定
- **Decision** — 评估结果：Allow / Deny / RequireApproval
- **Policy** — 策略规则集，分为内置预设和自定义策略

## 命令列表

| 命令 | 说明 |
|------|------|
| `cosh-cli audit check --action <cmd>` | 评估命令安全性 |
| `cosh-cli audit log` | 查看审计日志 |
| `cosh-cli audit policy show` | 显示当前策略 |

## check

评估一条 shell 命令是否被策略允许。

```bash
cosh-cli audit check --action "rm -rf /var/log"
```

输出：

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

安全命令示例：

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

查看审计日志条目。

```bash
cosh-cli audit log --session abc123
```

审计日志默认写入 `$COSH_AUDIT_LOG` 指定路径，未设置时使用
`~/.copilot-shell/audit.log`。

## policy show

显示当前生效的审计策略。

```bash
cosh-cli audit policy show
```

## 内置策略预设

| 预设 | 说明 |
|------|------|
| `permissive` | 宽松模式，大多数操作 Allow |
| `balanced` | 平衡模式（默认），写操作需要审批 |
| `strict` | 严格模式，几乎所有非只读操作 Deny |

## 策略加载优先级

1. 环境变量 `COSH_AUDIT_POLICY` 指定的策略文件
2. `~/.copilot-shell/cosh/audit.toml`（用户级）
3. `/etc/cosh/audit.toml`（系统级）
4. 内置 `balanced` 预设

## 动作解析

`parse_action_string()` 将原始 shell 字符串解析为结构化 `Action`：

- 按空白字符（空格、Tab、换行）分词
- 检测 shell 元字符（`;` `|` `&` `>` `<` `$` `` ` `` `(` `)` `{` `}`）
- 含元字符的命令直接拒绝（无法安全分析）

## 日志脱敏

审计日志写入前自动对敏感字段脱敏：

- 密码参数（`--password`、`-p` 后的值）
- API 密钥和 token
- 环境变量中的 secret 值

## Decision 枚举

| 决定 | 含义 | Agent 行为 |
|------|------|-----------|
| `Allow` | 策略允许 | 直接执行 |
| `Deny` | 策略拒绝 | 中止，不执行 |
| `RequireApproval` | 需要人工确认 | 暂停，等待用户审批 |
