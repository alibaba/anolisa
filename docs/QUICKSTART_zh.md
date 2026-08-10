# ANOLISA 快速入门

[English](QUICKSTART.md)

ANOLISA 是面向 AI Agent 工作负载的服务端操作系统层。通过统一的 `anolisa` CLI 安装和管理所有组件，为 Agent 提供 Token 优化、工作区快照、可观测性、安全策略、持久记忆等能力。

---

## 安装 CLI

```bash
curl -fsSL https://get.agentic-os.sh | bash
```

> Alinux 4 用户也可通过 `sudo yum install anolisa` 安装。

验证安装。

```bash
anolisa --version
```

AgentSecCore 的 raw 包需要 `anolisa` 0.2.17 或更高版本。先更新 CLI，
它才能读取当前的组件契约，并正确选中已发布的 raw 包。

```bash
# 通过 get.agentic-os.sh 安装的 CLI
anolisa update self

# 由 RPM 管理的 CLI
sudo anolisa update self
```

---

## 环境探测与组件安装

```bash
# 查看环境支持情况
anolisa env

# 列出可用组件
anolisa list
```

按需安装组件。当前 `cosh-ng`、`agentsight`、`sec-core`、`ws-ckpt` 和
`skillfs` 需要以 system 模式安装，其余示例支持 user 模式。

```bash
# Token 优化
anolisa install tokenless

# 工作区快照（基于 btrfs COW）
sudo anolisa --install-mode system install ws-ckpt

# 可观测性（Linux system mode）
sudo anolisa --install-mode system install agentsight

# 安全运行时和 adapter 资源（Linux x86_64 system mode）
sudo anolisa --install-mode system install sec-core

# 持久记忆（MCP 文件形态）
anolisa install agent-memory

# 技能文件系统（FUSE 虚拟视图）
sudo anolisa --install-mode system install skillfs

# OS 技能库
anolisa install os-skills

# Copilot Shell
anolisa install cosh

# cosh-ng（AI 原生 Linux 终端）
sudo anolisa --install-mode system install cosh-ng
```

检查健康状态。

```bash
anolisa status
```

安装会放置 systemd 服务文件，不会自动启动或设置开机自启。准备开始
采集时，再把 AgentSight 交给 systemd 管理。

```bash
sudo systemctl enable --now agentsight.service
sudo systemctl status agentsight.service
```

主服务会一起启动 trace 和 Dashboard，并带起它依赖的
`agentsight-enforcer.service`。

---

## 使用各组件

安装后，各组件可以独立使用。

```bash
# 启动已安装的终端
cosh

# Token 优化，压缩工具定义和命令输出
tokenless compress-schema -f tool.json
tokenless env-check --all

# 工作区快照，快速创建和回滚
ws-ckpt checkpoint -w ~/project -s v1 -m "initial"
ws-ckpt rollback -w ~/project -s v1

# 可观测性，system 服务以 root 身份保存采集数据
sudo agentsight token --period week
# Web Dashboard：http://localhost:7396

# 安全，系统加固与技能验证
agent-sec-cli harden --scan --config agentos_baseline
agent-sec-cli skill-ledger status
```

---

## 适配 Agent 框架

将已安装组件接入 Agent 框架，如 cosh、OpenClaw 或 Hermes。

```bash
anolisa adapter scan                        # 发现已安装框架
anolisa adapter enable tokenless openclaw   # tokenless → OpenClaw
anolisa adapter enable ws-ckpt hermes       # ws-ckpt → Hermes
anolisa adapter enable sec-core openclaw    # AgentSecCore → OpenClaw
```

---

## 下一步

### 全局入口

- [完整用户指南](user-guide/zh/README.md)，按分类目录浏览所有组件文档
- [安装指南](user-guide/zh/installation.md)，从 CLI 到全栈的渐进式安装
- [故障排查](user-guide/zh/troubleshooting.md)，查看常见问题与修复方法

### 用户入口点

- [anolisa CLI 命令参考](user-guide/zh/user-entrypoint/anolisa-cli.md)
- [cosh-ng AI 原生终端](user-guide/zh/user-entrypoint/cosh-ng/QUICKSTART.md)
- [Copilot Shell](user-guide/zh/user-entrypoint/copilot-shell/QUICKSTART.md)
- [OS 技能库](user-guide/zh/user-entrypoint/os-skills.md)

### 运行时与 Token 节省

- [工作区快照](user-guide/zh/runtime/ws-ckpt.md)
- [技能文件系统](user-guide/zh/runtime/skillfs.md)
- [Token 优化](user-guide/zh/token-saving/tokenless/QUICKSTART.md)
- [Agent 记忆](user-guide/zh/token-saving/agent-memory.md)

### 可观测性与安全

- [AgentSight](user-guide/zh/agent-observability/agentsight.md)
- [AgentSecCore](user-guide/zh/agent-security/agent-sec-core/QUICKSTART.md)
