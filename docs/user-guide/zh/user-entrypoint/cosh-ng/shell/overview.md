# cosh-shell 总览

cosh-shell 是 cosh-ng 的 AI 增强交互式终端。它在原生 bash/zsh PTY 之上叠加
AI 分析能力、工具审批控制和内联卡片渲染，为用户提供安全、可观测的 Agent
交互体验。

## 定位

cosh-shell 是面向终端用户的前端层：

- 管理 PTY 主机（bash/zsh 子进程）
- 通过 AI 适配器连接后端（默认 cosh-core）
- 渲染审批卡片和 AI 分析结果
- 实施工具审批控制协议

## 运行模式

```bash
# 默认启动（使用配置中的适配器和 shell）
cosh-shell

# 显式指定适配器（位置参数）
cosh-shell raw cosh-core
cosh-shell raw claude
cosh-shell raw qwen

# 指定底层 shell
cosh-shell --shell zsh
cosh-shell raw co --shell bash

# 直通模式：执行单条命令后退出
cosh-shell -c 'ls -la'
cosh-shell -- git status

# 登录 shell 模式
cosh-shell --login
cosh-shell -l

# 隔离模式（跳过用户 rcfile）
cosh-shell --isolated
```

## 支持的 AI 适配器

| 适配器 | 后端 | 说明 |
|--------|------|------|
| `cosh-core` | cosh-core 进程 | 默认适配器，完整控制协议 |
| `claude` | Claude Code CLI | Claude 适配器 |
| `qwen` | Qwen Code CLI | 通义千问适配器 |
| `fake` | 模拟 | 开发测试用，无需后端 |

## 适配器能力

| 能力 | 说明 |
|------|------|
| `text_stream` | 文本流式输出 |
| `thinking_stream` | 思考过程流式输出 |
| `session_resume` | 会话恢复 |
| `tool_intent` | 工具调用意图感知 |
| `user_question` | 向用户提问 |
| `cancellable` | 支持取消运行中的请求 |
| `control_protocol` | 完整控制协议支持 |

## 核心功能

| 功能 | 说明 | 详细文档 |
|------|------|----------|
| PTY 交互 | bash/zsh 原生终端 | [interactive-mode.md](interactive-mode.md) |
| AI 分析 | 流式命令分析 | [ai-analysis.md](ai-analysis.md) |
| 工具审批 | 可视化审批卡片 | [approval.md](approval.md) |

## 架构概览

```
┌────────────────────────────────────────────┐
│                 cosh-shell                 │
│  ┌───────────┐  ┌──────────┐  ┌─────────┐  │
│  │ PTY Host  │  │ Adapter  │  │   UI    │  │
│  │ (bash/zsh)│  │(cosh-core│  │(ratatui)│  │
│  └───────────┘  │/claude..)│  └─────────┘  │
│  ┌───────────┐  └──────────┘  ┌─────────┐  │
│  │  Hooks    │  ┌──────────┐  │Approval │  │
│  │  Engine   │  │  Tools   │  │ Broker  │  │
│  └───────────┘  └──────────┘  └─────────┘  │
└────────────────────────────────────────────┘
         │                │
         ▼                ▼
    bash/zsh PTY     cosh-core 进程
```

## 配置

cosh-shell 特有配置位于 `~/.copilot-shell/config.toml` 的 `[ui]` 和 `[shell]`
段。详见 [配置文档](../configuration.md)。

## 项目信任

cosh-shell 维护项目级信任存储。首次在新项目目录启动时，提示用户确认是否信任该项目。信任状态决定：

- 是否加载项目目录下的 `.cosh/hooks`
- 是否应用项目级配置覆盖
