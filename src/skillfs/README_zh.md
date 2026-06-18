# SkillFS

[English](README.md) | **中文**

基于 FUSE 的本地技能文件系统，负责解析 `SKILL.md`、按 view 组织技能，并把编译后的 `SKILL.md` 通过 FUSE 文件系统暴露出来。

[![Rust](https://img.shields.io/badge/Rust-1.86+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

## 能力

- 解析标准的 `SKILL.md`。
- 支持平铺目录和分类目录加载。
- 通过 `skillfs-views.toml` 管理默认视图和 secondary views。
- FUSE 目录中显示 primary view 技能。
- 始终暴露 `skill-discover`，用于列出 secondary views 中的技能及其源文件路径。
- 读取 `SKILL.md` 时执行条件编译和命令归一化。
- 透传 skill 目录内的物理文件和子目录。
- 支持 normal mount 与 in-place mount。
- 支持挂载后的物理写入透传，并把 `SKILL.md` 变化同步回 store。

## 功能矩阵

| 操作 | normal mount | in-place mount | 说明 |
|------|--------------|----------------|------|
| `readdir` | 虚拟视图 | 虚拟视图 | 由 views + store 决定可见 skill |
| 读 `SKILL.md` | 编译后返回 | 编译后返回 | 走 `compiler::compile` |
| 读其他文件 | 透传 | 透传 | 直接读取物理文件 |
| 写 `SKILL.md` | 透传 + store reparse | 透传 + store reparse | 目录名是 store 权威 key |
| `create` 普通文件 | 透传 | 透传 | 不触发 store 更新 |
| `mkdir` skill 目录 | 立即可见 | 立即可见 | 先插入 degraded placeholder |
| `rename` skill 目录 | 立即切换可见性 | 立即切换可见性 | 无空窗，旧名立即移除 |
| `unlink` `SKILL.md` | 从 store 移除 | 从 store 移除 | skill 从虚拟视图消失 |
| `rmdir` skill 目录 | 从 store 移除 | 从 store 移除 | 递归清理 inode 映射 |
| `setattr(size)` | 支持 truncate | 支持 truncate | 其他控制属性不作为主要能力 |
| `mknod` / `symlink` / `link` | `EROFS` | `EROFS` | 保持拒绝 |

## 范围

- 运行入口是 `mount`、`classify`、`validate`、`list`。
- 技能可见性由 `skillfs-views.toml` 决定。
- FUSE 当前已经支持挂载后的写 passthrough，但只有 `SKILL.md` 会触发 store 同步。
- store 的权威 key 是目录名，不再信任重命名后可能滞后的 frontmatter `name:`。

## 架构

```text
physical skills dir
  └─ skill-name/SKILL.md
            │
            ▼
    skillfs-core
      - parser
      - store
      - views
      - compiler
            │
            ▼
      skillfs-fuse
            │
            ▼
     mounted /skills view
```

## 写路径与一致性

SkillFS 现在不是纯只读文件系统，而是"虚拟目录视图 + 物理写透传"的混合模型：

- `readdir` 仍由虚拟视图控制。
- `SKILL.md` 读取仍返回编译后的内容，而不是原始文件。
- 其他文件读写直接落到底层文件系统。
- `SKILL.md` 的写入、创建、rename 后写入会通过后台 sync worker 重新解析并更新 `SharedSkillStore`。
- `mkdir` / `rename` skill 目录走立即一致路径，先同步更新 store，再由异步 reparse 覆盖为真实条目。
- in-place mount 通过 `/proc/self/fd/{n}` 访问底层 source，避免 over-mount 自回环。

## 快速开始

### 构建

```bash
cargo build --release
```

### 常用命令

```bash
# 验证 skills
cargo run -p skillfs -- validate /path/to/skills

# 列出 skills
cargo run -p skillfs -- list /path/to/skills

# 生成或查看 skillfs-views.toml
cargo run -p skillfs -- classify /path/to/skills

# 挂载 FUSE 文件系统
cargo run -p skillfs -- mount /path/to/skills /path/to/mountpoint
```

### `skillfs-views.toml`

技能选择依赖 `skillfs-views.toml`：

```toml
[[view]]
name = "major"
default = true
description = "Skills shown directly in /skills"
skills = ["github", "notion", "slack"]

[[view]]
name = "other"
default = false
description = "Skills exposed via skill-discover"
skills = ["apple-notes", "blogwatcher"]
```

挂载后：

- `/skills` 中显示默认 view 技能。
- `skill-discover/SKILL.md` 中列出 secondary views 的技能和 `source_path`。

## `SKILL.md` 格式

```markdown
---
name: my-skill
description: Brief description
version: 1.0.0
tags: [tooling, example]
enabled: true
---

# My Skill

Detailed instructions.

## Parameters

- `input` (string, required): Input value
- `options` (object, optional): Extra options

## Returns

- `result` (string, required): Result value
```

## 条件编译

FUSE 读取 `SKILL.md` 时会执行 `compiler::compile`，支持：

- `<!-- @if os == darwin -->`
- `<!-- @if has_command("uv") -->`
- `<!-- @else -->`
- `<!-- @endif -->`

没有条件块时，也会做少量启发式命令归一化，例如：

- `pip install` -> `uv pip install`
- `python -m venv` -> `uv venv`
- `npm install` -> `pnpm install` / `yarn install`

## 项目结构

```text
crates/
  skillfs-core/   parser, store, views, compiler, env, watcher
  skillfs-fuse/   FUSE 文件系统
  skillfs-cli/    mount / classify / validate / list
docs/specs/       实现说明
scripts/          保留 build.sh 与 test.sh
```

## 测试脚本

- [scripts/build.sh](scripts/build.sh)
  - 统一执行 workspace 构建。
- [scripts/test.sh](scripts/test.sh)
  - 创建临时 skill 源目录和 `skillfs-views.toml`。
  - 验证 FUSE 挂载成功。
  - 验证 `/skills` 暴露 default view 技能。
  - 验证 `skill-discover` 正确列出 secondary view 与 `source_path`。
  - 验证 skill 目录中的物理文件透传。
  - 验证通过 `SIGTERM` 可以正确卸载。

## 测试覆盖

`crates/skillfs-fuse/tests/write_guard_tests.rs` 当前覆盖：

- normal mount：读 `SKILL.md`、写透传、`mkdir` 立即可见、`rename` 无空窗、post-rename stale frontmatter 不复活旧名。
- in-place mount：`mkdir` 立即可见、`rename` 无空窗、post-rename stale frontmatter 不复活旧名。
- 拒绝操作：`mknod` / `symlink` / `link` 返回 `EROFS`。

`skillfs-core` 当前覆盖：

- parser、store、watcher 的单元与集成测试。

## 功能亮点

- 虚拟视图与物理文件系统解耦：目录展示由 views 控制，文件内容仍来自真实 source。
- `SKILL.md` 读写分离：读取给 agent 编译结果，写入落到原始文件。
- rename 后目录名是统一权威 key，避免 stale frontmatter 把旧 skill 名重新注入 store。
- in-place mount 通过 dir fd 绕过 FUSE 自身，避免写回进入自循环。

## 文档

- [docs/specs/skillfs-spec.md](docs/specs/skillfs-spec.md) - 整体架构、运行时一致性边界、场景对比
- [docs/specs/core-spec.md](docs/specs/core-spec.md) - `skillfs-core` 实现
- [docs/specs/fuse-spec.md](docs/specs/fuse-spec.md) - `skillfs-fuse` 实现

## 验证

下列命令是 CI 等价检查；本地在提交 PR 前跑一遍可以缩短反馈环。
任何一条不通过的改动都不应被合入。

```bash
# 1. 格式化 — 必须无 diff。
cargo fmt --all --check

# 2. Clippy — 在 -D warnings 下必须零 warning。
cargo clippy --workspace --all-targets -- -D warnings

# 3. workspace 内的单元与集成测试。
cargo test --workspace

# 4. 端到端 FUSE 挂载测试（需要 fuse3 + /dev/fuse；
#    在 macOS 或无 /dev/fuse 的容器中脚本会自动跳过）。
scripts/test.sh

# 5. Rustdoc — 修改公共 API 或 doc 注释时必跑，其余情况建议跑；
#    可在第一时间发现 intra-doc link 失效。
cargo doc --workspace --no-deps
```

完整贡献者约定（注释风格、模块布局、依赖策略、错误处理、commit 规范）
见 [AGENTS.md](AGENTS.md)。
