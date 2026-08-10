# cosh-ng 用户手册

[English](../../../en/user-entrypoint/cosh-ng/README.md)

cosh-ng 是一个 AI 原生 Linux 终端，让日常 Shell 操作和 Agent 任务在同一个终端中完成。先阅读快速开始，再按下面的任务导航查找所需功能或命令。

## 从这里开始

- [快速开始](QUICKSTART.md)：安装 cosh-ng 并完成第一个任务。
- [模型提供商](core/providers.md)：配置认证并选择模型提供商。
- [配置](configuration.md)：了解配置文件、设置项和优先级。
- [支持的平台](supported-distros.md)：确认软件包和服务后端。

## 在终端工作

| 目标 | 继续阅读 |
|---|---|
| 在同一会话中运行 Shell 命令和自然语言任务 | [交互式终端](shell/overview.md) |
| 选择 Agent 工具调用何时需要确认 | [工具审批](shell/approval.md) |
| 恢复或压缩会话 | [会话恢复](shell/session-recovery.md) |
| 了解斜杠命令和按键行为 | [交互行为](shell/interactive-mode.md) |

## 添加可复用能力

| 目标 | 继续阅读 |
|---|---|
| 在项目或团队之间共享操作说明 | [Skills](core/skills.md) |
| 接入本地进程或远程服务提供的工具 | [接入 MCP 服务](mcp.md) |
| 打包 Skills、Hooks、设置和工具 | [Extensions](core/extensions.md) |
| 在 Agent 生命周期事件前后运行检查 | [Hooks](core/hooks.md) |

## 管理系统操作

先运行只读命令。对支持的包管理或服务变更先加 `--dry-run` 预览；这类操作通常需要 root 权限。

| 目标 | 继续阅读 |
|---|---|
| 查找、安装或删除软件包 | [软件包管理](cli/package-management.md) |
| 查看或修改 systemd 服务 | [服务管理](cli/service-management.md) |
| 保存、比较、恢复或清理工作区快照 | [工作区快照](cli/checkpoint.md) |
| 查看策略决策和审计事件 | [安全审计](cli/audit.md) |

## 集成与自动化

- [结构化 OS CLI](cli/overview.md)：命令域和安全的自动化方式。
- [输出格式](output-format.md)：`CoshResponse<T>` 成功和失败响应封装。
- [无界面模式](core/headless-mode.md)：供其他前端使用的 JSONL 集成。
- [Agent 工具](core/tools.md)：工具边界和审批行为。
