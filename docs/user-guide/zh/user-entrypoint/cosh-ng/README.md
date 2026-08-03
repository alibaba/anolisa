# cosh-ng 用户手册

[English](../../../en/user-entrypoint/cosh-ng/README.md)

cosh-ng 把日常 Shell 操作和 Agent 任务放在同一个 Linux 终端里。这份手册先按你要完成的
工作给出入口，需要查命令或集成细节时再进入参考页。

## 从这里开始

- [安装并完成第一个任务](QUICKSTART.md)
- [选择模型 Provider 并登录](core/providers.md)
- [了解配置文件和覆盖顺序](configuration.md)
- [确认支持的平台](supported-distros.md)

## 在终端中工作

| 你要完成的事 | 继续阅读 |
|---|---|
| 混合使用 Shell 命令和自然语言任务 | [交互式终端](shell/overview.md) |
| 决定哪些 Agent 操作需要确认 | [工具审批](shell/approval.md) |
| 接着处理之前的工作 | [会话恢复](shell/session-recovery.md) |
| 让长会话保持在模型窗口内 | [会话压缩](shell/session-compaction.md) |
| 查询 slash 命令和按键行为 | [交互行为](shell/interactive-mode.md) |

## 加入可复用能力

| 你要加入的能力 | 继续阅读 |
|---|---|
| 团队或项目共享的操作说明 | [Skills](core/skills.md) |
| 本地进程或远程服务提供的工具 | [接入 MCP server](mcp.md) |
| 打包好的 Skills、Hooks、设置和工具 | [Extensions](core/extensions.md) |
| 在 Agent 事件前后执行的检查 | [Hooks](core/hooks.md) |

## 管理系统操作

先运行只读命令。操作支持 `--dry-run` 时，先看预览结果。修改软件包和服务通常需要
root 权限。

| 任务 | 继续阅读 |
|---|---|
| 安装、删除或查找软件包 | [包管理](cli/package-management.md) |
| 查看或修改 systemd 服务 | [服务管理](cli/service-management.md) |
| 保存和恢复工作区状态 | [工作区快照](cli/checkpoint.md) |
| 检查策略和审计记录 | [安全审计](cli/audit.md) |

## 集成与自动化

- [结构化 OS CLI](cli/overview.md)介绍稳定的 JSON 接口。
- [输出格式](output-format.md)说明成功和失败响应。
- [Headless 模式](core/headless-mode.md)面向其他前端和 JSONL 集成。
- [Agent 工具](core/tools.md)说明工具边界和审批行为。
