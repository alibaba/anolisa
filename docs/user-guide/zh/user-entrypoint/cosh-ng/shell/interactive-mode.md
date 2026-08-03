# 交互行为和命令

[English](../../../../en/user-entrypoint/cosh-ng/shell/interactive-mode.md)

本页是启动 cosh 和控制当前会话的精简参考。请运行 `/help` 查看已安装版本实际支持的
命令集合。

## 启动模式

| 命令 | 行为 |
|---|---|
| `cosh` | 启动已配置的 bash/zsh 和 Agent 适配器 |
| `cosh --shell zsh` | 选择底层 Shell |
| `cosh --isolated` | 跳过用户 rcfile，启动隔离会话 |
| `cosh --login` | 启动 login shell |
| `cosh --resume [id]` | 选择持久化的 Agent 对话 |
| `cosh -c '<command>'` | 通过底层 Shell 执行后退出 |
| `cosh -- <program> [args...]` | 直接执行程序后退出 |

## 输入和编辑

- Shell 语法会发送到前台 bash/zsh 进程。
- 在 `smart` 或 `auto` 分析模式下，自然语言输入会开始一轮 Agent 对话。
- 行首斜杠会调用 cosh 控制命令。自然语言句子里即使出现斜杠，也不会被当成控制命令。
- 终端支持协商后的按键协议时，`Shift+Enter` 可插入换行；多行粘贴保持为一次逻辑提交。
- 上方向键历史同时包含 Shell 输入和 slash 命令。

cosh 会向子 Shell 注入私有 OSC 消息，用它标记命令边界。这样无需解析 Shell
提示符，也能关联命令文本、退出状态、工作目录和捕获到的输出。

## 公开 slash 命令

| 命令 | 用途 |
|---|---|
| `/help` | 查看运行版本支持的命令 |
| `/draft` | 打开 prompt draft 工作流 |
| `/health` | 运行本地健康检查 |
| `/status`（`/about`） | 查看运行状态、provider 和当前会话；`/about` 是别名 |
| `/stats [model\|tools]` | 查看当前模型或工具活动 |
| `/auth` | 选择或更新 provider 认证 |
| `/config language [auto\|en-US\|zh-CN]` | 查看或设置 UI 语言 |
| `/mode approval [recommend\|auto\|trust]` | 查看或修改审批行为 |
| `/mode analysis [smart\|auto\|manual]` | 查看或修改分析路由 |
| `/session ...` | 新建、列出（包括 `--all`）、恢复、清理或压缩对话 |
| `/recommendations [on\|off\|status\|privacy\|clear]` | 管理本地输入建议 |
| `/hooks <command>` | 查看 Hook 记录、反馈和项目信任状态 |
| `/extensions <command>` | 管理扩展包和设置 |
| `/skills [list\|detail\|enable\|disable]` | 管理可复用 Skills |
| `/mcp [list\|connect\|inspect\|refresh\|disconnect\|login\|logout]` | 管理 MCP server |

`/details`、`/audit`、`/send-to-shell` 等命令只在当前卡片或任务具备所需上下文时出现。
诊断和兼容命令不会出现在普通帮助中。

## Skills

```text
/skills detail service-health
/skills disable service-health
/skills enable service-health
```

`/skills` 需要默认的 cosh-core 适配器。Skill 状态按启动 cosh 时确定的工作空间解析，
并从下一轮 Agent 对话开始生效。

## 终端恢复

cosh 会在正常退出、panic、`SIGTERM`、`SIGHUP` 或 `SIGQUIT` 时恢复终端设置。如果被
强制终止后终端仍显示异常，可在父 Shell 中运行 `reset`。

对话持久化和删除保证见[会话恢复](session-recovery.md)，卡片行为见[工具审批](approval.md)。
