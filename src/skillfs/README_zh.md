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
- 为普通 passthrough 路径提供 Linux POSIX 兼容基线，包括
  fd-backed I/O、create/mkdir mode 处理、长路径 fallback、
  open-after-unlink 句柄、受限 symlink/hardlink 策略、FIFO 创建、
  以及保守的 `user.*` xattr 透传。
- 提供可选的外部安全集成面：decision-command activation、
  activation 文件/xattr 消费、notify socket 事件、protocol JSONL
  事件、active mapping reload、startup reconcile，以及可信写进程
  身份校验。

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
| `symlink` | 受限透传 | 受限透传 | 仅允许同 skill 内的相对目标 |
| `link` | 受限透传 | 受限透传 | 仅允许同 skill 内普通文件 |
| `mkfifo` | 透传 | 透传 | 仅 FIFO；device node 仍拒绝 |
| `xattr user.*` | 透传 | 透传 | 仅普通 passthrough 路径 |

## 安全集成

SkillFS 不在文件系统核心中实现安全判断。外部 provider 负责扫描、
认证和策略判断，然后告诉 SkillFS 应该暴露哪个运行视图：

- `current`：暴露当前 source；
- `fallback`：暴露可信 `.skill-meta/versions/*.snapshot`；
- `hidden`：不在 `/skills` 中暴露该 skill。

当前支持两条集成路径：

- 兼容模式：通过 `--security --decision-command`，SkillFS 依次执行
  `<decision-command> scan <skill_dir> --json` 和
  `<decision-command> resolve <skill_dir> --json`；
- activation 模式：通过 `--activation-mode file`，SkillFS 消费
  `.skill-meta/activation.json` 或
  `user.agent_sec.skill_ledger.activation`，向外部 daemon 发送 notify
  事件，并在 activation 变化后刷新 active mapping。

对于 in-place security mount，`--ledger-backing-root` 提供 daemon
可读写的 source 视图，避免 daemon 读取 agent 可见的 FUSE 视图。
`--trusted-writer-exe` 可以通过可信进程的 executable 路径和
`(dev, ino)` 身份授权部分 `.skill-meta/**` 写入；进程名匹配仅保留为
兼容 fallback。

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
  skillfs-fuse/   FUSE 文件系统与 POSIX passthrough 层
  skillfs-cli/    mount / classify / validate / list
docs/specs/       实现说明
docs/testing/     POSIX 验收与外部 harness 文档
scripts/          build.sh、test.sh 与可选 POSIX harness
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
- `.skill-meta/**` 与虚拟路径的元数据保护边界。

额外 FUSE integration suites 覆盖 POSIX passthrough 行为：

- open/read/write flag 处理与 fd-backed I/O；
- create/mkdir mode 与 umask 行为；
- PATH_MAX fallback 与 open-after-unlink 句柄；
- 同 skill 内 symlink / hardlink 策略；
- FIFO 创建与 device node 拒绝；
- `user.*` xattr get/list/set/remove 行为。

可选 pjdfstest harness 位于
[scripts/posix/run_pjdfstest.sh](scripts/posix/run_pjdfstest.sh)，
操作说明见
[docs/testing/posix-external-harness.md](docs/testing/posix-external-harness.md)。

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
- [docs/specs/posix-phase1-spec.md](docs/specs/posix-phase1-spec.md) - POSIX passthrough 基线
- [docs/testing/posix-phase1-acceptance.md](docs/testing/posix-phase1-acceptance.md) - POSIX 验收清单
- [docs/security/external-decision-protocol.md](docs/security/external-decision-protocol.md) - decision-command JSON 协议
- [docs/security/runtime-activation-implementation-plan.md](docs/security/runtime-activation-implementation-plan.md) - activation / notify / reload / backing-root 集成说明
- [docs/skillfs-filesystem-capability-record.md](docs/skillfs-filesystem-capability-record.md) - 长期维护的能力记录
- [POSIX_FS_TEST_MATRIX.csv](POSIX_FS_TEST_MATRIX.csv) - POSIX 测试矩阵与当前覆盖

## 验证

下列命令是 CI 等价检查；本地在提交 PR 前跑一遍可以缩短反馈环。
任何一条不通过的改动都不应被合入。SkillFS 代码改动提交前必须
先通过格式检查和 clippy。

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
