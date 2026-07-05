# ANOLISA 用户指南

ANOLISA 为 AI Agent 提供完整的服务端运行时能力。通过 `anolisa` CLI 统一安装，各组件独立使用。

---

## 组件全景

```
┌────────────────────────────────────────────────────────────────────┐
│  Agent 应用层（cosh / OpenClaw / Hermes / 自定义）                 │
├────────────────────────────────────────────────────────────────────┤
│  用户入口点                                                        │
│  anolisa-cli · cosh · os-skills                                    │
├──────────────────────────────────┬─────────────────────────────────┤
│  Token 节省                       │  运行时                          │
│  tokenless · agent-memory        │  skillfs · ws-ckpt              │
├──────────────────────────────────┼─────────────────────────────────┤
│  Agent 可观测                     │  Agent 安全                      │
│  agentsight                      │  agent-sec-core                 │
└──────────────────────────────────┴─────────────────────────────────┘
```

---

## 文档目录

### 全局入口

| 文档 | 内容 |
|------|------|
| [安装与初始化](installation.md) | 从 CLI 到全栈组件的渐进式安装 |
| [故障排查](troubleshooting.md) | 跨模块常见问题与修复方案 |

### 用户入口点 `user-entrypoint/`

| 文档 | 组件 | 说明 |
|------|------|------|
| [anolisa CLI](user-entrypoint/anolisa-cli.md) | anolisa | 统一 CLI 组件管理 |
| [Copilot Shell](user-entrypoint/copilot-shell.md) | cosh | AI 终端助手与命令网关 |
| [OS 技能库](user-entrypoint/os-skills.md) | os-skills | 系统管理与 DevOps 技能 |

### 可观测性 `agent-observability/`

| 文档 | 组件 | 说明 |
|------|------|------|
| [AgentSight](agent-observability/agentsight.md) | agentsight | eBPF 追踪、Token 计账、Web Dashboard |

### 安全 `agent-security/`

| 文档 | 组件 | 说明 |
|------|------|------|
| [AgentSecCore](agent-security/agent-sec-core.md) | agent-sec-core | 系统加固、代码扫描、提示词扫描、技能账本 |

### Token 节省 `token-saving/`

| 文档 | 组件 | 说明 |
|------|------|------|
| [Token 优化](token-saving/tokenless.md) | tokenless | Schema/响应压缩、命令重写 |
| [Agent 记忆](token-saving/agent-memory.md) | agent-memory | MCP 持久化文件形态记忆 |

### 运行时 `runtime/`

| 文档 | 组件 | 说明 |
|------|------|------|
| [工作区快照](runtime/ws-ckpt.md) | ws-ckpt | 秒级快照创建/回滚，基于 btrfs COW |
| [技能文件系统](runtime/skillfs.md) | skillfs | FUSE 虚拟视图、渐进披露 |

---

## 术语速查

| 术语 | 含义 |
|------|------|
| 组件（Component） | 实现某项功能的软件单元，如 `tokenless` |
| 适配器（Adapter） | 将组件接入 Agent 框架的桥接包 |
| system mode | 需要 root 权限的安装模式（`sudo anolisa install`） |
| user mode | 安装到用户目录，无需 sudo |
