# 工作区快照（ws-ckpt）

ws-ckpt 为 AI Agent 提供毫秒级工作区快照和回滚能力。它利用文件系统 COW（Copy-on-Write）技术创建即时快照，支持安全实验和快速恢复。

---

## 概述

AI Agent 修改代码、配置或数据文件时，误操作代价高昂。ws-ckpt 允许 Agent（和用户）：

- 在风险操作前创建即时快照
- 毫秒内回滚到任意历史检查点
- 比较检查点之间的差异
- 基于可配置触发器自动创建检查点

---

## 前置条件

- Linux（x86_64 或 aarch64）
- 工作区所在卷使用 btrfs 文件系统（用于 COW 快照）
- Agent 运行时：OpenClaw 或 Hermes（Plugin 模式）

---

## 安装

### 方式一：anolisa CLI（推荐）

```bash
anolisa install ws-ckpt
```

### 方式二：YUM（Alinux，需配置 ANOLISA YUM 源）

```bash
sudo yum install ws-ckpt
```

### 方式三：源码编译（开发者）

```bash
cd src/ws-ckpt && make build
```

---

## 插件安装

为你的 Agent 运行时安装 ws-ckpt 插件：

```bash
# OpenClaw
ws-ckpt plugin install --runtime openclaw

# Hermes
ws-ckpt plugin install --runtime hermes
```

---

## Skill 安装

启用自然语言驱动的检查点操作，安装 ws-ckpt skill：

```bash
# 从 GitHub 安装
# Skill 地址：https://github.com/alibaba/anolisa/blob/main/src/ws-ckpt/src/skills/ws-ckpt/SKILL.md
```

安装后，Agent 可以理解如下自然语言指令：
- "保存当前工作区状态"
- "回滚到上一个检查点"
- "展示上次保存后的变更"

---

## CLI 命令

| 命令 | 说明 |
|------|------|
| `ws-ckpt checkpoint [--name <name>]` | 创建新检查点 |
| `ws-ckpt rollback <checkpoint-id>` | 将工作区恢复到指定检查点 |
| `ws-ckpt list` | 列出所有检查点 |
| `ws-ckpt diff <id1> [<id2>]` | 显示检查点之间的差异 |
| `ws-ckpt delete <checkpoint-id>` | 删除指定检查点 |
| `ws-ckpt status` | 查看当前工作区状态 |
| `ws-ckpt config` | 查看/编辑配置 |

### 示例

```bash
# 创建命名检查点
ws-ckpt checkpoint --name "before-refactor"

# 列出已有检查点
ws-ckpt list

# 对比当前状态与某个检查点
ws-ckpt diff ckpt-3a7f

# 回滚到特定检查点
ws-ckpt rollback ckpt-3a7f

# 删除旧检查点
ws-ckpt delete ckpt-1b2c
```

---

## 自然语言用法（Agent 驱动）

安装 ws-ckpt skill 后，Agent 可通过自然语言操作检查点：

| 意图 | 示例表达 |
|------|----------|
| 创建检查点 | "保存工作区"、"开始前先做个快照" |
| 回滚 | "撤销所有修改"、"恢复到上一个好的状态" |
| 列出检查点 | "显示所有保存的状态"、"列出我的检查点" |
| 差异对比 | "上次保存后改了什么？" |

---

## 自动检查点

ws-ckpt 支持自动创建检查点：

```toml
# ~/.config/ws-ckpt/config.toml

[auto_checkpoint]
# 每次 Agent 调用工具前自动创建检查点
on_tool_call = true

# 定时检查点（cron 表达式）
schedule = "*/10 * * * *"   # 每 10 分钟

[cleanup]
# 自动清理超过 N 小时的检查点
max_age_hours = 24

# 最大保留检查点数量
max_count = 50
```

---

## 重要注意事项

> **警告**：ws-ckpt 配置的工作区路径**不能**是：
> - Agent 启动目录或其父目录
> - 系统路径（`/`、`/usr`、`/etc`、`/var`）
>
> 将工作区设置为上述路径可能导致系统不稳定或 Agent 故障。

---

## 配置

默认配置文件路径：`~/.config/ws-ckpt/config.toml`

```toml
[workspace]
# 管理的工作区路径
path = "/home/user/projects/my-project"

[storage]
# 快照存储后端
backend = "btrfs"

[auto_checkpoint]
on_tool_call = true
schedule = ""

[cleanup]
max_age_hours = 24
max_count = 50
```

---

## 常见问题

**Q：文件系统不是 btrfs 怎么办？**
A：ws-ckpt 会回退到基于 rsync 的快照方式，速度较慢但功能等效。

**Q：能同时管理多个工作区吗？**
A：可以。运行 `ws-ckpt config` 管理多个工作区路径。

**Q：检查点占用多少磁盘空间？**
A：使用 btrfs COW 时，仅存储变更的块。每个检查点的典型开销 < 工作区大小的 5%。
