# 交互式终端

[English](../../../../en/user-entrypoint/cosh-ng/shell/overview.md)

`cosh`是一个bash或zsh终端，也可以接收Agent的自然语言任务。熟悉的命令直接使用Shell语法；需要排查或执行较大任务时，再用自然语言描述目标。

## 典型工作流

1. 进入目标目录并运行`cosh`。
2. 像平常一样执行熟悉的命令。
3. 用自然语言描述排查或任务，并写明“仅检查”“修改前询问”等约束。
4. 允许副作用前检查审批卡片。
5. 离开长时间排查前运行`/session status`。

常用启动方式：

```bash
cosh
cosh --shell zsh
cosh --resume
```

## 输入如何分流

| 输入 | 结果 |
|---|---|
| `git status` | 在前台Shell中执行。 |
| `why did the last command fail?` | 携带最近终端证据启动Agent请求。 |
| `/session list` | 执行cosh控制命令。 |
| Agent工具请求 | 按审批模式自动执行或显示审批卡片。 |

获批的Shell命令仍在前台Shell中执行，因此提示、输出、任务控制和`Ctrl+C`都可用。安全规则见[工具审批](approval.md)。

## 会话与主动帮助

- 会话由cosh-core保存，并按启动cosh时所在工作空间隔离。恢复会话只恢复模型可见的对话内容，不恢复终端进程或旧终端输出。详见[会话恢复](session-recovery.md)。
- `smart`是默认分析模式。需要调整主动的失败帮助时，请看[AI分析](ai-analysis.md)。
- `/help`是已安装版本命令集合的准确信息；简要参考见[交互命令](interactive-mode.md)。

## 下一步

- [工具审批](approval.md)
- [AI分析](ai-analysis.md)
- [会话恢复](session-recovery.md)
- [会话压缩](session-compaction.md)
- [Skills](../core/skills.md)
- [MCP](../mcp.md)
- [Extensions](../core/extensions.md)
