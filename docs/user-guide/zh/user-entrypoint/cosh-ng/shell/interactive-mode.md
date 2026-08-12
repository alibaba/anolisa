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
| `/agent` | 编辑一次性 cosh-core 请求，可选择 Skill 并引用工作空间路径。 |
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

## 编辑一次性 Agent 请求

配置使用 cosh-core runtime 时，可以运行 `/agent`。Agent Composer 会打开多行
编辑器，但不会改变后续 Shell 输入的路由方式。按 Enter 发送请求，按
`Shift+Enter`插入换行，按`Esc`取消并恢复 Shell prompt。

第一个 token 可以选择一个 Skill，后续任何以`@`开头、由空白分隔的 token 都可以
引用当前工作空间内的文件或目录。

```text
/skill:repo-review inspect @Cargo.toml @src
```

- `/skill:<name>`必须是第一个 token。所选 Skill 会在其他 Agent tool 之前调用。
- `@路径`必须指向工作空间内现有的文件或目录。绝对路径、父目录跳转、通过符号链接
  逃出工作空间的路径和不支持的路径都会被拒绝。
- 每次提交最多接受 16 个有效引用。目录默认不递归，除非请求明确要求遍历。
- cosh-ng 只发送验证后的路径元数据，在构建请求时不会读取或嵌入文件内容。
- Agent turn 开始前，界面会显示每个被拒绝引用的路径和原因；这些引用不会进入
  结构化引用上下文。

不指定 Skill 或引用的纯文本请求同样有效。其他 provider runtime 不提供`/agent`；
此时可直接输入自然语言请求或使用多行输入。

审批行为见[工具审批](approval.md)，主动的失败帮助见[AI分析](ai-analysis.md)。
