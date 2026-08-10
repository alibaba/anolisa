# ANOLISA 用户指南

[English](../en/README.md)

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
| [cosh-ng](user-entrypoint/cosh-ng/README.md) | cosh-ng | 集成 Agent runtime 的 AI 原生 Linux 终端 |
| [Copilot Shell](user-entrypoint/copilot-shell/QUICKSTART.md) | cosh | AI 终端助手与命令网关 |
| [OS 技能库](user-entrypoint/os-skills.md) | os-skills | 系统管理与 DevOps 技能 |

### 可观测性 `agent-observability/`

| 文档 | 组件 | 说明 |
|------|------|------|
| [AgentSight](agent-observability/agentsight.md) | agentsight | eBPF 追踪、Token 计账、Web Dashboard |

### 安全 `agent-security/`

| 文档 | 组件 | 说明 |
|------|------|------|
| [AgentSecCore](agent-security/agent-sec-core/QUICKSTART.md) | agent-sec-core | 系统加固、代码扫描、提示词扫描、技能账本 |
| [Code Scanner Hook 配置](agent-security/agent-sec-core/code-scanner.md) | agent-sec-core | 各 Agent 的 hook 模式、环境变量与 fallback 行为 |
| [PII 检测](agent-security/agent-sec-core/pii-checker.md) | agent-sec-core | 个人数据/凭证检测与脱敏 |
| [Skill Ledger 用户指南](agent-security/agent-sec-core/skill-ledger.md) | agent-sec-core | 技能账本完整性链与签名工作流 |
| [OpenClaw 兼容部署与升级](agent-security/agent-sec-core/openclaw-deploy.md) | agent-sec-core | OpenClaw 插件部署与升级指南 |

### Token 节省 `token-saving/`

| 文档 | 组件 | 说明 |
|------|------|------|
| [Tokenless 快速开始](token-saving/tokenless/QUICKSTART.md) | tokenless | 安装、接入 Agent、首次压缩与验收 |
| [Tokenless 用户手册](token-saving/tokenless/user-manual.md) | tokenless | 能力边界、运行行为与任务导航 |
| [Tokenless 框架集成](token-saving/tokenless/framework-integration.md) | tokenless | cosh、OpenClaw、Hermes、Qoder、Claude Code、Codex、Qwen Code |
| [Tokenless CLI 参考](token-saving/tokenless/cli-reference.md) | tokenless | 压缩、环境检查、Stash、MCP 与统计命令 |
| [Tokenless 效果度量](token-saving/tokenless/measuring-savings.md) | tokenless | 统计、diff、dry-run、AgentSight 与 SLS 度量 |
| [Tokenless 配置与数据隐私](token-saving/tokenless/configuration-and-privacy.md) | tokenless | 配置优先级、本地数据与敏感工作负载 |
| [Tokenless 故障排查](token-saving/tokenless/troubleshooting.md) | tokenless | Adapter、数据库、Stash、升级与卸载 |
| [Agent 记忆](token-saving/agent-memory.md) | agent-memory | 持久化记忆、MCP 工具、检索与数据主权控制 |

### 运行时 `runtime/`

| 文档 | 组件 | 说明 |
|------|------|------|
| [Blaze Sandbox Runtime](runtime/blaze.md) | blaze | Managed sandbox 的可选 VM 网络与周期存储制品同步 |
| [工作区快照](runtime/ws-ckpt.md) | ws-ckpt | 秒级快照创建/回滚，基于 btrfs COW |
| [技能文件系统](runtime/skillfs.md) | skillfs | FUSE 虚拟视图、渐进披露 |
| [SkillFS Kubernetes Sidecar](runtime/skillfs-kubernetes-sidecar.md) | skillfs | 在 Kubernetes 中以 FUSE Sidecar 运行 SkillFS |

---

## 术语速查

| 术语 | 含义 |
|------|------|
| 组件（Component） | 实现某项功能的软件单元，如 `tokenless` |
| 适配器（Adapter） | 将组件接入 Agent 框架的桥接包 |
| system mode | 需要 root 权限的安装模式（`sudo anolisa install`） |
| user mode | 安装到用户目录，无需 sudo |
