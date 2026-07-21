# ANOLISA 快速入门

[English](QUICKSTART.md)

ANOLISA 是面向 AI Agent 工作负载的服务端操作系统层。通过统一的 `anolisa` CLI 安装和管理所有组件，为 Agent 提供 Token 优化、工作区快照、可观测性、安全策略、持久记忆等能力。

---

## 安装 CLI

```bash
curl -fsSL https://agentic-os.sh | sh
```

> Alinux 4 用户也可通过 `sudo yum install anolisa` 安装。

验证：

```bash
anolisa --version
```

---

## 环境探测与组件安装

```bash
# 查看环境支持情况
anolisa env

# 列出可用组件
anolisa list
```

按需安装组件。大部分组件使用 user mode；`agentsight` 和 `agent-sec-core` 需要 system mode（sudo）。

```bash
# Token 优化
anolisa install tokenless

# 工作区快照（基于 btrfs COW）
anolisa install ws-ckpt

# 可观测性（需要 sudo — eBPF 依赖 root）
sudo anolisa install agentsight

# 安全内核（需要 sudo）
sudo anolisa install agent-sec-core

# 持久记忆（MCP 文件形态）
anolisa install agent-memory

# 技能文件系统（FUSE 虚拟视图）
anolisa install skillfs

# OS 技能库
anolisa install os-skills

# Copilot Shell（AI 终端网关）
anolisa install cosh
```

检查健康状态：

```bash
anolisa status
```

---

## 使用各组件

安装后，各组件独立使用：

```bash
# Copilot Shell — AI 终端助手
cosh

# Token 优化 — 压缩 tool schema 和命令输出
tokenless compress-schema -f tool.json
tokenless env-check --all

# 工作区快照 — 秒级创建/回滚
ws-ckpt checkpoint -w ~/project -s v1 -m "initial"
ws-ckpt rollback -w ~/project -s v1

# 可观测性 — 追踪 Agent Token 消耗
sudo agentsight trace
agentsight token --period week
agentsight serve   # Web Dashboard: http://localhost:7396

# 安全 — 系统加固与技能验证
agent-sec-cli harden --scan --config agentos_baseline
agent-sec-cli skill-ledger status
```

---

## 适配 Agent 框架

将已安装组件桥接到 Agent 框架（cosh / OpenClaw / Hermes）：

```bash
anolisa adapter scan                        # 发现已安装框架
anolisa adapter enable tokenless openclaw   # tokenless → OpenClaw
anolisa adapter enable ws-ckpt hermes       # ws-ckpt → Hermes
```

---

## 下一步

### 全局入口

- [完整用户指南](user-guide/zh/README.md) — 按分类目录浏览所有组件文档
- [安装指南](user-guide/zh/installation.md) — 从 CLI 到全栈的渐进式安装
- [故障排查](user-guide/zh/troubleshooting.md) — 常见问题与修复

### 用户入口点

- [anolisa CLI 命令参考](user-guide/zh/user-entrypoint/anolisa-cli.md)
- [Copilot Shell](user-guide/zh/user-entrypoint/copilot-shell/QUICKSTART.md)
- [OS 技能库](user-guide/zh/user-entrypoint/os-skills.md)

### 运行时与 Token 节省

- [工作区快照](user-guide/zh/runtime/ws-ckpt.md)
- [技能文件系统](user-guide/zh/runtime/skillfs.md)
- [Token 优化](user-guide/zh/token-saving/tokenless/QUICKSTART.md)
- [Agent 记忆](user-guide/zh/token-saving/agent-memory/QUICKSTART.md)

### 可观测性与安全

- [AgentSight](user-guide/zh/agent-observability/agentsight.md)
- [AgentSecCore](user-guide/zh/agent-security/agent-sec-core/QUICKSTART.md)
