# 交互式终端

[English](../../../../en/user-entrypoint/cosh-ng/shell/overview.md)

`cosh` 是 cosh-ng 的主要入口。它保留普通 bash 或 zsh 会话，同时让 Agent
也能接收同一输入行中的自然语言任务。Agent 的处理过程会持续输出，工具调用会按需
显示审批卡片。Skills、MCP 工具和可恢复对话都可以在这个终端中使用。

## 输入怎样分流

cosh 会根据输入内容决定交给 Shell 还是 Agent。

| 输入 | cosh 的处理方式 |
|---|---|
| `git status` | 原样发送到前台 Shell |
| `上一个命令为什么失败？` | 携带最近终端证据启动 Agent turn |
| `/session list` | 执行 cosh 控制命令 |
| Agent 调用工具 | 按审批模式显示卡片或自动执行 |

交互式命令仍由前台 Shell 执行。Agent 建议的 Shell 命令经过审批后会交回前台
PTY。命令输出、交互提示、任务控制和 `Ctrl+C` 都会照常工作。

## 启动和恢复

```bash
cosh                         # 使用已配置的 Shell 和 cosh-core adapter
cosh --shell zsh             # 明确选择 zsh
cosh --isolated              # 跳过用户 rcfile
cosh --resume                # 选择当前工作空间的对话
cosh --resume <session-id>   # 恢复已知对话
cosh -c 'uname -a'           # 非交互 passthrough
```

底层 Shell 依次参考 `--shell`、`COSH_SHELL_RAW_SHELL`、`shell.default` 和当前用户的
login shell。这些信息都无法确定 Shell 时，使用 bash。

## 日常工作流

1. 进入目标目录并运行 `cosh`。
2. 已经知道的命令继续使用 Shell 语法。
3. 使用自然语言描述更高层任务，并写明“仅检查”“修改前询问”等约束。
4. 允许副作用前仔细检查审批卡片。
5. 把可复用指令整理为 Skill。
6. 离开长时间排查任务前运行 `/session status` 或 `/status`。

## 控制入口

`/help` 会列出已安装版本实际支持的命令。常用命令可以按用途查找。

| 目标 | 命令 |
|---|---|
| Runtime 和健康状态 | `/status`、`/health`、`/stats [model\|tools]` |
| 认证与模式 | `/auth`、`/mode approval ...`、`/mode analysis ...`、`/config language ...` |
| 对话生命周期 | `/session ...`、`/draft` |
| 可复用能力 | `/skills ...`、`/extensions ...`、`/mcp ...` |
| 检查 | `/hooks`、`/recommendations ...` |

完整公开命令摘要见[交互行为](interactive-mode.md)，模式语义见[工具审批](approval.md)。

## 对话持久化

Agent 对话由 cosh-core 保存，并按启动 cosh 时确定的工作空间隔离。使用
`cosh --resume` 或 `/session resume <id>` 恢复。恢复后，Agent 可以继续使用原对话内容，
终端进程和临时 UI 状态不会恢复。

## 安全边界

- `recommend` 会在每次 Agent 工具调用前询问。
- 默认的 `auto` 可自动执行符合条件的低风险工具，但 Shell 命令和受保护操作仍需审批。
- `trust` 会移除常规审批，因此必须显式执行 `/mode approval trust confirm` 才能进入。
- 项目 Hook 和工作空间扩展设置只对受信任的项目根目录生效。
- 审计和诊断输出会在导出前脱敏。

## 下一步

- [交互行为和 slash 命令](interactive-mode.md)
- [AI 分析模式](ai-analysis.md)
- [工具审批](approval.md)
- [会话恢复](session-recovery.md)
- [会话压缩](session-compaction.md)
- [Skills](../core/skills.md)
- [接入 MCP server](../mcp.md)
- [Extensions](../core/extensions.md)
- [配置](../configuration.md)
