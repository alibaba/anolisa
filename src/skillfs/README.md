# SkillFS

基于 FUSE 的本地技能文件系统，负责解析 `SKILL.md`、按 view 组织技能，并把编译后的 `SKILL.md` 通过 FUSE 文件系统暴露出来。

[![Rust](https://img.shields.io/badge/Rust-1.86+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

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
- 支持主流 POSIX/FUSE 读写语义：fd-backed I/O、metadata、目录流、
  rename、PATH_MAX fallback、open-after-unlink、safe symlink/link、FIFO、
  `user.*` xattr。
- 提供安全集成基础设施：`.skill-meta` 保护、audit JSONL、security mount
  mode、install inbox、trusted writer (exe identity)、activation file/xattr、notify、
  protocol event log、runtime reload 和 startup reconcile。

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
| `symlink` | 策略允许 | 策略允许 | 允许相对 same-skill；拒绝绝对、跨 skill、`.skill-meta`、lifecycle |
| `link` | 策略允许 | 策略允许 | 允许 same-skill regular file；拒绝跨 skill 和特殊文件 |
| `mknod` | FIFO only | FIFO only | block/char/socket/device node 仍拒绝 |
| xattr | `user.*` | `user.*` | no-follow passthrough；非 user namespace 拒绝 |

## 安全集成能力

SkillFS 不直接实现扫描、签名校验或风险判断。它提供文件系统层能力，让外部
安全组件决定一个 skill 应该暴露为 current、fallback snapshot，还是 hidden。

当前已支持：

- `.skill-meta/**` 默认 mutation-protected，普通 mount-path 写入返回
  deterministic permission error。
- `--audit-log <PATH>` 输出稳定 JSONL audit stream。
- `--security-mode` 要求 source 和 mountpoint 指向同一目录，用于更强的
  in-place enforcement。
- `/.skillfs-inbox/<skill>/...` 作为安装候选入口；写入落到 source，完成信号
  触发后续安全流程。
- `--decision-command <COMMAND>` 兼容路径：变更后 debounce，执行
  `<COMMAND> scan <skill_dir> --json`，再执行
  `<COMMAND> resolve <skill_dir> --json`，刷新 active mapping。
- `--activation-mode file` 生产路径：从
  `.skill-meta/activation.json` 或
  `user.agent_sec.skill_ledger.activation` xattr 读取 activation。
- `--notify-socket <PATH>` 发送 skill 变更通知给外部 daemon。
- `--activation-events-log <PATH>` 写入 activation protocol event JSONL。
- `--activation-reload-mode poll` 在 notify 后轮询 activation 并刷新 resolver。
- startup reconcile 会在挂载后对已知 skill 发出 best-effort reconcile 通知。
- `--trusted-writer-exe <PATH>`（推荐）生产级可信写门禁，按
  `/proc/<tgid>/exe` readlink + 文件身份 `(dev, ino)` 匹配，结合 starttime
  做 PID reuse 防护，抗 `prctl PR_SET_NAME` 伪造。
- `--trusted-writer <NAME>`（deprecated / 兼容性）按 Linux TGID `comm` 匹配；
  进程 `comm` 可被伪造，不应用于生产。两者同时配置时 exe 为授权依据。

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

    SkillFS 现在不是纯只读文件系统，而是“虚拟目录视图 + 物理写透传”的混合模型：

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

`crates/skillfs-fuse/tests/` 当前覆盖：

- normal / in-place mount 的 `SKILL.md` 编译读、写透传、store reparse、
  mkdir/rename/unlink/rmdir 可见性和 stale frontmatter 防回归。
- POSIX open/create、metadata、目录流、PATH_MAX fallback、open-after-unlink、
  safe symlink/link/FIFO、`user.*` xattr。
- `.skill-meta`、lifecycle、security mode、audit runtime、source drift、
  install inbox、trusted writer、activation consumer、notify、runtime reload、
  startup reconcile。
- 外部 pjdfstest harness 可选运行，正常 `cargo test` 不依赖 pjdfstest。

`skillfs-core` 当前覆盖：

- parser、store、watcher 的单元与集成测试。

## 功能亮点

- 虚拟视图与物理文件系统解耦：目录展示由 views 控制，文件内容仍来自真实 source。
- `SKILL.md` 读写分离：读取给 agent 编译结果，写入落到原始文件。
- rename 后目录名是统一权威 key，避免 stale frontmatter 把旧 skill 名重新注入 store。
- in-place mount 通过 dir fd 绕过 FUSE 自身，避免写回进入自循环。
- active mapping 支持把 `/skills/<name>` 映射到当前 source、可信 snapshot 或
  hidden，已打开 fd 保持 open-time target pinning。

## 文档

- [HANDOFF.md](HANDOFF.md) - 代码状态与后续建议
- [docs/specs/skillfs-v1-spec.md](docs/specs/skillfs-v1-spec.md) - 整体架构说明
- [docs/specs/core-spec.md](docs/specs/core-spec.md) - `skillfs-core` 实现
- [docs/specs/fuse-spec.md](docs/specs/fuse-spec.md) - `skillfs-fuse` 实现
- [docs/skillfs-filesystem-capability-record.md](docs/skillfs-filesystem-capability-record.md) - 文件系统能力、POSIX 兼容、安全集成与演进记录
- [docs/specs/posix-phase1-spec.md](docs/specs/posix-phase1-spec.md) - POSIX Phase 1 行为规格
- [docs/testing/posix-external-harness.md](docs/testing/posix-external-harness.md) - 外部 POSIX harness 使用说明
- [docs/testing/posix-phase1-acceptance.md](docs/testing/posix-phase1-acceptance.md) - Phase 1 验收条件
- [POSIX_FS_TEST_MATRIX.csv](POSIX_FS_TEST_MATRIX.csv) - POSIX 文件系统测试矩阵
- [POSIX_FS_REFERENCES.md](POSIX_FS_REFERENCES.md) - POSIX / FUSE / 本仓库参考资料

## 验证

```bash
cargo test -p skillfs-core
cargo test -p skillfs-fuse
cargo check -p skillfs -p skillfs-fuse
```
