# Agent Memory

Agent Memory 为 AI Agent 提供基于 MCP 的持久化文件记忆能力。它通过将结构化记忆存储为文件，使 Agent 能跨会话保留上下文，并通过 Model Context Protocol（MCP）访问。

---

## 概述

AI Agent 通常在会话间丢失所有上下文。Agent Memory 通过以下方式解决此问题：

- **持久化存储** — 记忆在 Agent 重启和会话间持续保留
- **文件架构** — 记忆以结构化文件形式存储，透明且可移植
- **MCP 接口** — 标准 Model Context Protocol 服务器，提供 30+ 工具，无缝集成 Agent
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

### 方式二：YUM（Alinux，需配置 ANOLISA YUM 源）

```bash
sudo yum install agent-memory
```

### 方式三：源码编译（开发者）

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

## MCP 工具

Agent Memory 提供 30+ MCP 工具，主要分类如下：

### 文件操作

- `mem_read` / `mem_write` / `mem_append` / `mem_edit` — 读取、写入、追加、编辑记忆文件
- `mem_list` / `mem_grep` / `mem_diff` — 列出、搜索、对比记忆内容
- `mem_mkdir` / `mem_remove` — 管理记忆目录和文件
- `mem_promote` — 提升记忆条目

### 会话与上下文

- `mem_session_log` — 记录会话活动
- `memory_search` / `memory_observe` / `memory_get_context` — 语义搜索和上下文获取
- `memory_sessions` / `memory_timeline` / `memory_summary` — 会话历史和摘要

### 维护

- `mem_dream` / `mem_consolidate` / `mem_compact` — 后台整合与压缩
- `mem_index_refresh` — 刷新记忆索引
- `mem_snapshot` / `mem_snapshot_list` / `mem_snapshot_restore` — 快照管理
- `mem_log` / `mem_revert` — 历史日志和回退

### 任务管理

- `memory_task_save` / `memory_task_resume` / `memory_task_list` / `memory_task_close` — 保存和恢复多步骤任务

### 导入/导出与元数据

- `mem_export` / `mem_import` — 批量导出和导入
- `memory_about` / `memory_forget` / `memory_auto_created` / `memory_consent` — 元数据和控制

---

## 配置

配置文件：`~/.anolisa/memory.toml`

该文件为**可选项**，不会自动生成。不存在时 Agent Memory 使用内置默认值。仅在需要覆盖默认行为时创建。

```toml
# 示例：覆盖默认值
[storage]
path = "~/.anolisa/memory/"

[server]
transport = "stdio"
```

### 数据目录

记忆文件默认存储于 `~/.anolisa/memory/`。

---

## 常见问题

**Q：记忆存储在哪里？**
A：默认存储在 `~/.anolisa/memory/`，以结构化文件形式保存。

**Q：配置文件是必需的吗？**
A：不是。Agent Memory 使用内置默认值即可工作。`~/.anolisa/memory.toml` 为可选配置，仅在需要覆盖特定设置时使用。

**Q：Agent Memory 能在沙箱环境中工作吗？**
A：可以。Agent Memory 设计为可在受限/沙箱执行环境中运行。

**Q：与 Tokenless 有何区别？**
A：Tokenless 压缩上下文中的信息以节省 Token。Agent Memory 将知识卸载到持久化存储，使其无需出现在上下文中。
