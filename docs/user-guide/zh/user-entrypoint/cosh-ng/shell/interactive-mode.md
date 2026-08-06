# 交互命令

[English](../../../../en/user-entrypoint/cosh-ng/shell/interactive-mode.md)

本页介绍如何启动`cosh`并控制当前会话。运行`/help`可查看已安装版本实际支持的命令。

## 启动`cosh`

| 命令 | 用途 |
|---|---|
| `cosh` | 启动已配置的bash或zsh和Agent适配器。 |
| `cosh --shell zsh` | 明确选择zsh。 |
| `cosh --isolated` | 跳过用户rcfile。 |
| `cosh --login` | 启动login shell。 |
| `cosh --resume [id]` | 打开会话选择器或恢复指定会话。 |
| `cosh -c '<command>'` | 通过Shell执行一条命令后退出。 |
| `cosh -- <program> [args...]` | 直接执行程序后退出。 |

未指定Shell时，`cosh`使用配置或检测到的bash/zsh，无法确定时回退到bash。

## 输入和编辑

- Shell语法会发送到前台bash或zsh。
- 自然语言请求会启动Agent请求；分析模式只控制主动的失败帮助，不影响显式请求。
- 行首的`/`会运行cosh控制命令；普通句子中的斜杠不会触发控制命令。
- 终端支持时，`Shift+Enter`插入换行；多行粘贴仍作为一次提交。
- 上方向键历史包含Shell输入和斜杠命令。按`Ctrl+C`可取消当前命令或Agent请求。

## 公开斜杠命令

| 命令 | 用途 |
|---|---|
| `/help` | 查看已安装版本支持的命令。 |
| `/draft` | 编辑多行Agent请求。 |
| `/health` | 运行本地健康检查。 |
| `/status`（`/about`） | 查看运行时、Provider和会话状态。 |
| `/stats [model\|tools]` | 查看模型身份或工具活动。 |
| `/auth` | 选择或更新Provider认证。 |
| `/config language [auto\|en-US\|zh-CN]` | 查看或设置界面语言。 |
| `/mode approval [recommend\|auto\|trust]` | 查看或修改工具审批。 |
| `/mode analysis [smart\|auto\|manual]` | 查看或修改主动分析。 |
| `/session ...` | 新建、列出、恢复、清理或压缩会话。 |
| `/recommendations [on\|off\|status\|privacy\|clear]` | 管理本地输入建议。 |
| `/hooks <command>` | 查看Hook发现和信任状态。 |
| `/extensions <command>` | 管理扩展包和设置。 |
| `/skills [list\|detail\|enable\|disable]` | 管理Skills。 |
| `/mcp [list\|connect\|inspect\|refresh\|disconnect\|login\|logout]` | 管理MCP服务器。 |

`/details`、`/audit`和`/send-to-shell`等命令只有在当前卡片或任务提供所需上下文时才会出现。`/mcp login`需要按MCP指南说明在Shell中完成OAuth流程。

审批行为见[工具审批](approval.md)，主动的失败帮助见[AI分析](ai-analysis.md)。
