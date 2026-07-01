# Agent Memory

Agent Memory 为 AI Agent 提供基于 MCP 的持久化文件记忆能力。它通过将结构化记忆存储为文件，使 Agent 能跨会话保留上下文，并通过 Model Context Protocol（MCP）访问。

---

## 概述

AI Agent 通常在会话间丢失所有上下文。Agent Memory 通过以下方式解决此问题：

- **持久化存储** — 记忆在 Agent 重启和会话间持续保留
- **文件架构** — 记忆以结构化文件形式存储，透明且可移植
- **MCP 接口** — 标准 Model Context Protocol 服务器，无缝集成 Agent
- **沙箱执行** — 在受限环境中安全运行

---

## 前置条件

- Linux（x86_64 或 aarch64）
- 兼容 MCP 的 Agent 运行时

---

## 安装

### 方式一：anolisa CLI（推荐）

```bash
anolisa install agent-memory
```

### 方式二：源码编译（开发者）

```bash
cd src/agent-memory && make build
```

---

## 快速开始

```bash
# 1. 安装 Agent Memory
anolisa install agent-memory

# 2. 启动 MCP 服务器
agent-memory serve

# 3. 配置 Agent 运行时连接到 MCP 服务器
#    （参见下方集成章节）
```

---

## 集成

Agent Memory 作为 MCP 服务器运行。配置 Agent 运行时进行连接：

```json
{
  "mcpServers": {
    "agent-memory": {
      "command": "agent-memory",
      "args": ["serve"]
    }
  }
}
```

Agent 随后可在对话中通过 MCP 工具读写记忆。

---

## 配置

配置文件：`~/.config/agent-memory/config.toml`

```toml
[storage]
# 记忆文件存储目录
path = "~/.local/share/agent-memory"

[server]
# MCP 服务器传输方式
transport = "stdio"
```

---

## 常见问题

**Q：记忆存储在哪里？**
A：默认存储在 `~/.local/share/agent-memory/`，以结构化文件形式保存。

**Q：Agent Memory 能在沙箱环境中工作吗？**
A：可以。Agent Memory 设计为可在受限/沙箱执行环境中运行。

**Q：与 Tokenless 有何区别？**
A：Tokenless 压缩上下文中的信息以节省 Token。Agent Memory 将知识卸载到持久化存储，使其无需出现在上下文中。
