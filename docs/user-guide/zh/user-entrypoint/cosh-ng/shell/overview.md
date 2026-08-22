# 交互式终端

[English](../../../../en/user-entrypoint/cosh-ng/shell/overview.md)

`cosh` 默认启动 Enhanced Assisted 模式。`◇ ` 前缀表示 Cosh 可能在 bash 或
zsh 执行前路由自然语言输入。在空提示符按 `Shift+Tab` 可进入 Enhanced
Shell-only（`◌ `）。如果要求不加载 Cosh Hook、不观察也不提供洞察，需要在
启动时选择 Native。

## 典型工作流

1. 进入目标目录并运行`cosh`。
2. 像平常一样执行熟悉的命令。
3. 普通输入应只交给 Shell 时，按 `Shift+Tab`。
4. 在 Assisted 模式描述任务，并在允许副作用前检查卡片。
5. 离开长时间排查前运行`/session status`。

常用启动方式：

```bash
cosh
cosh --shell zsh
cosh --resume
COSH_SHELL_INTEGRATION=native cosh
```

## 输入如何分流

| 输入 | Native | Enhanced Shell-only `◌` | Enhanced Assisted `◇` |
|---|---|---|---|
| `git status` | 在 Shell 中执行。 | 在 Shell 中执行，之后可能提供执行洞察。 | 在 Shell 中执行，之后可能提供执行洞察。 |
| `hello` | Shell 通常报告命令不存在。 | Shell 通常报告命令不存在。 | 分类器会检查它，当前仍把这个有歧义的单词交给 Shell。 |
| `why did the last command fail?` | 由 Shell 处理。 | 由 Shell 处理。 | 携带最近终端证据启动 Agent 请求。 |
| `/session list` | 由 Shell 处理。 | 由 Shell 处理。 | 执行 Cosh 控制命令。 |
| Agent 工具请求 | 不可用。 | 明确接受洞察或进入 Agent 后可用。 | 按审批模式执行或显示审批卡片。 |

Native 不会安装 Cosh `DEBUG`、`RETURN` 或 `ERR` trap，也不会开启
`extdebug`、`functrace` 或 `errtrace`。Enhanced 是默认集成；使用
`shell.integration = "native"` 或 `COSH_SHELL_INTEGRATION=native` 选择
Native。切换集成需要重新启动 `cosh`，`Shift+Tab` 只切换 Enhanced 内部的
路由子状态，不需要重启。

增强集成中获批的 Shell 命令仍在前台 Shell 执行，prompt、输出、任务控制和
`Ctrl+C` 都可用。安全规则见[工具审批](approval.md)。

## 会话与主动帮助

- 增强会话由 cosh-core 保存，并按启动 cosh 时所在工作空间隔离。恢复会话只
  恢复模型可见的对话内容，不恢复终端进程或旧终端输出。详见
  [会话恢复](session-recovery.md)。
- `smart` 是增强集成中的默认分析模式。调整主动失败帮助的方法见
  [AI 分析](ai-analysis.md)。
- `/help` 是增强集成命令集合的准确信息，简要参考见
  [交互命令](interactive-mode.md)。

## 下一步

- [工具审批](approval.md)
- [AI分析](ai-analysis.md)
- [会话恢复](session-recovery.md)
- [会话压缩](session-compaction.md)
- [Skills](../core/skills.md)
- [MCP](../mcp.md)
- [Extensions](../core/extensions.md)
