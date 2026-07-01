# SkillFS

SkillFS 是基于 FUSE 的虚拟文件系统，通过 default view + skill-discover 控制可见 Skill，将精选的 Agent 技能以 `/skills` 视图暴露给 Agent。

## 概述

| 能力 | 说明 |
|------|------|
| 视图可见性控制 | 默认视图的 Skill 出现在 `/skills`；其他通过 `skill-discover` 访问 |
| 条件编译 | 读取 `SKILL.md` 时执行编译（OS 条件、命令归一化） |
| 物理写透传 | 非 SKILL.md 文件直接透传到底层文件系统 |
| skill-discover | 虚拟技能，列出 secondary view 中的技能及其源文件路径 |

## 前置条件

| 条件 | 最低要求 |
|------|----------|
| OS | Linux |
| 文件系统 | FUSE3（`libfuse3-dev` 或等价包） |
| 设备 | `/dev/fuse` 必须可用 |
| Rust | >= 1.86（仅源码编译） |

## 安装

```bash
# 首选
anolisa install skillfs

# 源码编译（仅开发者）
cd src/skillfs && cargo build --release
```

## 快速开始

```bash
# 验证源目录中的 skills
skillfs validate /path/to/skills

# 列出所有 skills
skillfs list /path/to/skills

# 生成 skillfs-views.toml（将技能分配到默认视图和 secondary 视图）
skillfs classify /path/to/skills

# 挂载虚拟文件系统
skillfs mount /path/to/skills /mnt/skillfs --foreground
# Agent 访问路径: /mnt/skillfs/skills/<skill-name>/SKILL.md
```

## 使用详解

### skillfs mount — 挂载虚拟文件系统

```bash
skillfs mount <SOURCE> <MOUNTPOINT> [OPTIONS]
```

- `SOURCE`：包含技能文件夹（每个含 `SKILL.md`）和 `skillfs-views.toml` 的目录
- `MOUNTPOINT`：SkillFS 暴露虚拟 `/skills` 视图的目录

挂载后，Agent 通过以下路径访问技能：
```
<MOUNTPOINT>/skills/<skill-name>/SKILL.md
```

`skill-discover` 虚拟技能始终存在：
```
<MOUNTPOINT>/skills/skill-discover/SKILL.md
```

关键选项：
- `--foreground`：前台运行（便于测试和 systemd）
- `--security-mode`：强制 in-place 挂载（SOURCE = MOUNTPOINT）
- `--audit-log <PATH>`：以 JSONL 格式追加文件系统审计事件
- `--pid-file <PATH>`：写入 PID 文件用于进程管理

示例：
```bash
# 开发模式挂载
skillfs mount ./skills /mnt/skillfs --foreground

# In-place 挂载 + 审计
skillfs mount ./skills ./skills --security-mode --audit-log /var/log/skillfs/audit.jsonl
```

### skillfs classify — 生成视图配置

```bash
skillfs classify <SOURCE> [--primary-count N] [--dry-run]
```

在源目录中生成 `skillfs-views.toml`。前 N 个技能放入默认视图（"major"），其余放入 secondary 视图（"other"）。

```bash
# 默认 6 个主要技能
skillfs classify /path/to/skills

# 预览不写入
skillfs classify /path/to/skills --dry-run

# 自定义数量
skillfs classify /path/to/skills --primary-count 10
```

### skillfs validate — 验证技能文件

```bash
skillfs validate <SOURCE> [--format text|json]
```

验证源目录中所有 `SKILL.md` 文件。报告 success、degraded（部分解析）和 error 状态。

```bash
# 文本输出（默认）
skillfs validate /path/to/skills

# JSON 输出（CI 集成）
skillfs validate /path/to/skills --format json
```

### skillfs list — 列出技能

```bash
skillfs list <SOURCE> [--enabled-only]
```

列出源目录中发现的所有技能及其元数据。

```bash
# 列出全部
skillfs list /path/to/skills

# 仅显示已启用的
skillfs list /path/to/skills --enabled-only
```

## 配置

SkillFS 使用 **源目录下** 的 `skillfs-views.toml`（不是全局配置路径）控制技能可见性：

```toml
[[view]]
name = "major"
default = true
description = "Core skills shown directly in /skills"
skills = ["github", "notion", "slack"]

[[view]]
name = "other"
default = false
description = "Additional skills accessible via skill-discover"
skills = ["apple-notes", "blogwatcher"]
```

行为：
- `default = true` 视图的技能出现在 `<mountpoint>/skills/`
- Secondary 视图列在 `skill-discover/SKILL.md` 中，每个技能附带 `source_path`
- 未分配到任何视图的技能在下次挂载时自动加入默认视图

## 架构

```
crates/
  skillfs-core/   parser, store, views, compiler, env, watcher
  skillfs-fuse/   FUSE 文件系统与 POSIX 透传层
  skillfs-cli/    mount / classify / validate / list
```

## 常见问题

**Q: 配置文件放在哪里？**

A: `skillfs-views.toml` 位于技能源目录中（即你作为 `<SOURCE>` 传入的那个目录），不在 `~/.config/` 下。

**Q: skill-discover 是什么？**

A: 它是始终存在于 `/skills` 中的虚拟技能。当存在 secondary views 时，它会列出这些技能及其 `source_path`，供 Agent 通过 `read_file` 直接访问。

**Q: Agent 能否通过 FUSE 挂载写入文件？**

A: 可以。非 SKILL.md 文件直接透传到物理文件系统。写入 `SKILL.md` 会触发重新解析以保持内部 store 一致。
