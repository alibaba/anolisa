# 工作区快照（ws-ckpt）

ws-ckpt 为 AI Agent 提供毫秒级工作区快照和回滚能力。它利用文件系统 COW（Copy-on-Write）技术创建即时快照，支持安全实验和快速恢复。

---

## 概述

AI Agent 修改代码、配置或数据文件时，误操作代价高昂。ws-ckpt 允许 Agent（和用户）：

- 在风险操作前创建即时快照
- 毫秒内回滚到任意历史检查点
- 比较检查点之间的差异
- 通过插件集成自动创建检查点

---

## 前置条件

- Linux（x86_64 或 aarch64）
- 工作区所在卷使用 btrfs 文件系统（用于原生 COW 快照），或任意文件系统（ws-ckpt 会自动创建 btrfs loop image）
- Agent 运行时：OpenClaw 或 Hermes（Plugin 模式）

---

## 安装

### 方式一：anolisa CLI（推荐）

```bash
sudo anolisa --install-mode system install ws-ckpt
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

# 卸载
ws-ckpt plugin uninstall --runtime openclaw
```

`plugin install` 会先执行 detect 脚本检查前置条件（exit 2 = 缺前置依赖，中止；exit 1 = 未安装但可安装，继续），通过后再执行 install 脚本。脚本位于 `/usr/share/anolisa/adapters/ws-ckpt/<runtime>/`。

---

## CLI 命令

| 命令 | 说明 |
|------|------|
| `ws-ckpt init -w <workspace>` | 初始化工作区 |
| `ws-ckpt checkpoint -w <workspace> -s <snapshot-id> -m <message> [--metadata <json>]` | 创建新检查点 |
| `ws-ckpt rollback -w <workspace> -s <snapshot> [--preview]` | 回滚到指定检查点 |
| `ws-ckpt rollback -w <workspace> -n <num-ancestors>` | 回滚 N 个祖先版本 |
| `ws-ckpt list [-w <workspace>] [--format table\|json]` | 列出所有检查点 |
| `ws-ckpt diff -w <workspace> -f <from> [-t <to>]` | 显示检查点间差异 |
| `ws-ckpt delete [-w <workspace>] -s <snapshot> [--force]` | 删除指定检查点 |
| `ws-ckpt status [-w <workspace>] [--format table\|json]` | 查看工作区状态 |
| `ws-ckpt cleanup -w <workspace> [--keep 20]` | 清理旧检查点 |
| `ws-ckpt config [-g \| -w <workspace>] [--enable-auto-cleanup] [--auto-cleanup-keep <N\|Nd>]` | 查看/编辑配置 |
| `ws-ckpt plugin install --runtime openclaw\|hermes` | 安装运行时插件 |
| `ws-ckpt plugin uninstall --runtime openclaw\|hermes` | 卸载运行时插件 |
| `ws-ckpt recover [-w <workspace> \| --all] [--force]` | 从中断操作中恢复 |
| `ws-ckpt reload` | 重载 daemon 配置 |
| `ws-ckpt daemon [--mount-path ...] [--socket ...] [--log-level ...]` | 启动 daemon 进程 |

### 示例

```bash
# 初始化工作区
ws-ckpt init -w /home/user/projects/my-project

# 创建检查点
ws-ckpt checkpoint -w /home/user/projects/my-project -s snap-001 -m "before refactor"

# 列出检查点
ws-ckpt list -w /home/user/projects/my-project

# 比较两个快照的差异
ws-ckpt diff -w /home/user/projects/my-project -f snap-001 -t snap-002

# 回滚到指定检查点
ws-ckpt rollback -w /home/user/projects/my-project -s snap-001

# 预览回滚（不实际执行）
ws-ckpt rollback -w /home/user/projects/my-project -s snap-001 --preview

# 清理旧检查点，保留最近 20 个
ws-ckpt cleanup -w /home/user/projects/my-project --keep 20

# 为工作区启用自动清理
ws-ckpt config -w /home/user/projects/my-project --enable-auto-cleanup --auto-cleanup-keep 7d
```

### diff 输出标记

| 标记 | 含义 | 颜色 |
|------|------|------|
| `+` | 新增文件/目录（Added） | 绿色 |
| `-` | 删除文件/目录（Deleted） | 红色 |
| `M` | 内容修改（Modified） | 黄色 |
| `R` | 重命名（Renamed） | 青色 |

> diff 内置智能解析器，自动将 btrfs 底层的临时 inode 引用（如 `o261-118-0`）解析为真实文件路径，并对同一文件的多个操作去重合并。预览回滚（`rollback --preview`）使用相同的标记含义。

---

## 配置

### Daemon 配置

daemon 配置文件位于 `/etc/ws-ckpt/config.toml`，为系统级 daemon 进程配置。

不存在用户侧全局配置文件。自动检查点和清理行为通过各插件配置控制：

### OpenClaw 插件配置

```json
// ~/.openclaw/ws-ckpt.json
{
  "autoCheckpoint": true,
  "workspace": "/home/user/projects/my-project"
}
```

### Hermes 插件配置

```bash
hermes config set plugins.ws-ckpt.workspace /home/user/projects/my-project
```

### CLI 配置

配置分两层：**全局**（`/etc/ws-ckpt/config.toml`，daemon-wide 默认值）与**局部**（per-workspace `policy.toml` 覆盖）。`ws-ckpt config` 不带 scope 时打印只读概览；`-g` 查看/修改全局；`-w` 仅可覆盖 `auto_cleanup` 与 `auto_cleanup_keep`，其余字段（interval / image / health check）为 daemon-wide，只能通过 `-g` 设置；`-w <workspace> --reset` 删除该工作区的覆盖，回退到沿用全局。

```bash
# 启用自动清理，保留 7 天内的检查点
ws-ckpt config -w /home/user/projects/my-project --enable-auto-cleanup --auto-cleanup-keep 7d

# 全局配置
ws-ckpt config -g --enable-auto-cleanup --auto-cleanup-keep 20
```

---

## 重要注意事项

> **警告**：ws-ckpt 配置的工作区路径**不能**是：
> - 根路径（`/`）
> - daemon mount_path 内部的路径
> - Agent 启动目录或其父目录（在 plugin 层校验）
>
> 这些约束由 daemon 代码强制执行。使用无效路径将被拒绝。

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

## 常见问题

**Q：文件系统不是 btrfs 怎么办？**
A：ws-ckpt 会在宿主文件系统上创建 btrfs loop image 并进行 loop mount，在任意文件系统类型上提供完整的 COW 快照功能。

**Q：能同时管理多个工作区吗？**
A：可以。每条命令通过 `-w` 指定工作区路径，或通过插件配置管理多个工作区。

**Q：检查点占用多少磁盘空间？**
A：使用 btrfs COW 时，仅存储变更的块。每个检查点的典型开销 < 工作区大小的 5%。
