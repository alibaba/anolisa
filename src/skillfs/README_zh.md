# SkillFS

[English](README.md) | **中文**

SkillFS 是面向 agent skill 的本地 FUSE 文件系统。它解析 `SKILL.md`，按 view
组织技能，并通过挂载后的文件系统暴露编译后的 `SKILL.md`，同时让普通 skill
文件继续由真实 source 树承载。

[![Rust](https://img.shields.io/badge/Rust-1.86+-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

## 能力

- 解析标准 `SKILL.md` 文件。
- 支持平铺 skill 目录和分类目录布局。
- 通过 `skillfs-views.toml` 选择默认 view 和 secondary views。
- 在挂载后的 agent 视图中直接显示默认 view 的 skills。
- 始终暴露虚拟 `skill-discover` skill，让 agent 能发现 secondary views 中的
  skills 及其 source paths。
- 读取 `SKILL.md` 时执行条件块编译和命令归一化。
- 将普通文件和子目录透传到底层物理 source 树。
- 支持 normal mount 和 in-place mount。
- 支持挂载期间的物理写透传；`SKILL.md` 变化会重新解析并更新内存 store。
- 为普通 passthrough 路径提供 Linux POSIX 兼容基线：fd-backed I/O、
  create/mkdir mode 处理、长路径 fallback、open-after-unlink 句柄、受限
  symlink/hardlink 策略、FIFO 创建，以及保守的 `user.*` xattr 透传。
- 提供可选外部安全集成面：decision-command activation、activation 文件/xattr
  消费、notify socket 事件、protocol JSONL 事件、active mapping reload、
  startup reconcile、可信写进程身份校验、可信 control socket 写入，以及
  managed mount recovery。

## 行为矩阵

| 操作 | normal mount | in-place mount | 说明 |
| --- | --- | --- | --- |
| `readdir` | 虚拟视图 | 虚拟视图 | 可见性由 views 和 store 决定。 |
| 读 `SKILL.md` | 编译后内容 | 编译后内容 | 使用 `compiler::compile`。 |
| 读其他文件 | 透传 | 透传 | 读取物理 source 文件。 |
| 写 `SKILL.md` | 透传 + store reparse | 透传 + store reparse | 目录名是 store 权威 key。 |
| `create` 普通文件 | 透传 | 透传 | 不更新 store。 |
| `mkdir` skill 目录 | 立即可见 | 立即可见 | 异步 reparse 前先插入 degraded placeholder。 |
| `rename` skill 目录 | 可见性立即切换 | 可见性立即切换 | 旧名无空窗移除。 |
| `unlink` `SKILL.md` | 从 store 移除 | 从 store 移除 | skill 从虚拟视图消失。 |
| `rmdir` skill 目录 | 从 store 移除 | 从 store 移除 | 同时清理 inode 映射。 |
| `setattr(size)` | 支持 truncate | 支持 truncate | 其他 metadata 操作在允许时保守透传。 |
| `symlink` | 受限透传 | 受限透传 | 仅允许同 skill 内相对目标。 |
| `link` | 受限透传 | 受限透传 | 仅允许同 skill 内普通文件。 |
| `mkfifo` | 透传 | 透传 | 仅 FIFO；device/socket node 拒绝。 |
| `xattr user.*` | 透传 | 透传 | 仅普通 passthrough 路径。 |

## 范围

- 公开 CLI 命令是 `mount`、`stop`、`classify`、`validate` 和 `list`。
- skill 可见性由 `skillfs-views.toml` 控制。
- FUSE 支持挂载期间写透传，但只有 `SKILL.md` 变化会触发 store 同步。
- 权威 skill key 是目录名，不是 rename 后可能滞后的 frontmatter `name:`。
- in-place mount 会 over-mount source 目录。受控的 skill 写入可以通过
  SkillFS 执行，但会 rename 或替换挂载目录本身的工具，例如 workspace
  checkpoint/init/rollback 工具，必须在 mount 前或 unmount 后执行。

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

SkillFS 是一个混合文件系统：虚拟目录视图 + 物理写透传。

- `readdir` 仍由虚拟 view 控制。
- 读取 `SKILL.md` 返回编译后内容，而不是原始 source 文件。
- 其他文件直接读写底层文件系统。
- 写入、创建或 rename 后写入 `SKILL.md` 会重新解析文件并更新
  `SharedSkillStore`。
- `mkdir` 和 skill 目录 `rename` 走立即一致路径，先同步更新 store，之后由
  异步 reparse 用真实条目替换 placeholder。
- in-place mount 使用 `/proc/self/fd/{n}` 访问底层 source，避免递归进入
  自己的 FUSE over-mount。

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

# opt-in managed mount：由脱离调用方的 supervisor 保持挂载存活
cargo run -p skillfs -- mount /path/to/skills /path/to/mountpoint --managed

# 停止 managed mount 并清除 desired mounted state
cargo run -p skillfs -- stop /path/to/mountpoint
```

### Managed Mount 模式

默认 `mount`（包括 `--foreground`）保持原有前台行为：进程阻塞，
`SIGTERM` 或 `Ctrl+C` 会干净卸载。如果启动它的进程（例如 gateway）重启并终止
子进程，挂载也会随之消失。

`--managed` 是 opt-in，用于需要跨 gateway restart 保持挂载的场景：

- client 写入 managed state，使用 `setsid` 在独立 session 中启动 detached
  supervisor，等待挂载 ready 后返回。
- supervisor 用相同的 source、mountpoint、config、security、audit、
  activation、trusted-writer、control socket 和 logging 选项启动前台 FUSE
  worker。
- 如果 worker 在 desired state 仍为 `mounted` 时意外退出，supervisor 会在有界
  backoff 后重新挂载。
- 只有 `skillfs stop <MOUNTPOINT>` 会清除 desired mounted state，终止
  supervisor/worker 并卸载。`stop` 是幂等的，对已经卸载的路径重复执行也是安全的。
- 如果 supervisor 被 `kill -9` 强制终止，可能留下仍在服务但无人监控的 orphan
  worker。重新启动 `mount --managed` 前，先执行 `skillfs stop <MOUNTPOINT>` 清理
  残留 state、process 和 mount。

managed state 存放在按用户隔离的运行时目录下：优先
`$XDG_RUNTIME_DIR/skillfs/`，其次 `/run/user/<uid>/skillfs/`，两者都不可用时
回退到 `/tmp/skillfs-<uid>/`。实例 id 从规范化后的 mountpoint 派生，因此
`mount` 与 `stop` 对同一挂载点始终指向同一实例。

### In-place Mount 与 Workspace Snapshot

SkillFS 使用 in-place mount 时，会替换挂载目录本身的工具需要在 mount 前或
unmount 后执行。例如，`ws-ckpt checkpoint -w <MOUNTPOINT>` 如果作用在活跃的
SkillFS mountpoint 上，可能会因为 `Device or resource busy` 失败。

通过 SkillFS 进行的写入，包括 install/update/remove skills，挂载期间仍然支持。

## `skillfs-views.toml`

skill 选择由 `skillfs-views.toml` 控制：

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

- `/skills` 显示 default view 中的 skills。
- `skill-discover/SKILL.md` 列出 secondary views 中的 skills 及其
  `source_path`。

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

FUSE 读取 `SKILL.md` 时，SkillFS 会执行 `compiler::compile`，支持：

- `<!-- @if os == darwin -->`
- `<!-- @if has_command("uv") -->`
- `<!-- @else -->`
- `<!-- @endif -->`

没有条件块时，SkillFS 也会执行少量启发式命令归一化，例如：

- `pip install` -> `uv pip install`
- `python -m venv` -> `uv venv`
- `npm install` -> `pnpm install` / `yarn install`

## 项目结构

```text
crates/
  skillfs-core/   parser, store, views, compiler, env, watcher
  skillfs-fuse/   FUSE 文件系统与 POSIX passthrough 层
  skillfs-cli/    mount / stop / classify / validate / list
docs/specs/       实现规格
docs/security/    external decision 与 runtime activation 文档
docs/testing/     POSIX 验收与 external harness 文档
docs/skills/      随仓库分发的 agent-facing SkillFS skill
scripts/          build.sh、test.sh 与可选 POSIX harness
```

## 测试脚本

- [scripts/build.sh](scripts/build.sh)
  - 执行 workspace build。
- [scripts/test.sh](scripts/test.sh)
  - 创建临时 skill source 目录和 `skillfs-views.toml`。
  - 验证 FUSE mount 启动成功。
  - 验证 `/skills` 暴露 default-view skills。
  - 验证 `skill-discover` 列出 secondary views 和 `source_path`。
  - 验证 skill 目录中物理文件的 passthrough read。
  - 验证通过 `SIGTERM` 干净卸载。
- [scripts/posix/run_pjdfstest.sh](scripts/posix/run_pjdfstest.sh)
  - 可选 external POSIX harness；普通 `cargo test` 不依赖它。

## 测试覆盖

`crates/skillfs-fuse/tests/` 覆盖：

- normal 和 in-place mount 下的 compiled `SKILL.md` read、write passthrough、
  store reparse、mkdir/rename/unlink/rmdir 可见性，以及 stale frontmatter 防回归；
- POSIX open/create、metadata、目录流、长路径 fallback、open-after-unlink、
  safe symlink/link/FIFO 和 `user.*` xattr；
- `.skill-meta`、lifecycle namespaces、security mode、audit runtime、source
  drift、install inbox、staging/direct install flows、trusted writer、trusted
  metadata view、activation consumer、control socket server behavior、notify、
  runtime reload、startup reconcile 和 post-publish grace paths。

`crates/skillfs-cli/tests/` 覆盖 CLI parsing 和 startup gates，包括 managed
mount supervision、activation/notify option compatibility、backing-root
requirements、trusted writer executable validation，以及 control-socket
trusted peer configuration。

`skillfs-core` 通过单元和集成测试覆盖 parser、store、compiler 与 watcher 行为。

## 功能亮点

- 虚拟 views 与物理文件系统解耦：目录可见性由 view 控制，文件内容仍来自真实
  source 树。
- `SKILL.md` 读写刻意分离：agent 读取编译后内容，写入更新原始 source 文件。
- rename 后目录名是统一权威 skill key，避免 stale frontmatter 把旧 skill 名重新注入。
- in-place mount 使用预打开 source dir fd，让 SkillFS 写透传时不会递归进入自己的
  FUSE mount。
- active mapping 可以把 `/skills/<name>` 暴露为 current source、trusted
  snapshot 或 hidden，已打开 fd 保持 open-time target pinning。

## 安全集成

SkillFS 不在文件系统核心中执行扫描、签名校验或风险判断。外部 provider 决定
一个 skill 应暴露为：

- `current`：服务 live source；
- `fallback`：服务可信 `.skill-meta/versions/*.snapshot`；
- `hidden`：从 agent-facing 视图隐藏该 skill。

当前支持两条集成路径：

- 兼容 decision-command 模式：
  `--security --decision-command <COMMAND>` 会执行
  `<COMMAND> scan <skill_dir> --json`，再执行
  `<COMMAND> resolve <skill_dir> --json`。
- activation-file 模式：
  `--security --activation-mode file` 消费
  `.skill-meta/activation.json` 或
  `user.agent_sec.skill_ledger.activation`；配置后会向外部 daemon 发送 notify
  事件，并在 activation 变化后重新加载 active mapping。

相关安全能力：

- `.skill-meta/**` 对不可信 lookup/list/read 路径隐藏，普通 mutation 会被拒绝。
  可信 exact-path access 可把 metadata 操作路由到 live source。
- `--audit-log <PATH>` 写稳定 JSONL audit events。
- `--security-mode` 要求 `SOURCE` 和 `MOUNTPOINT` 指向同一目录，使普通
  userspace 访问都经过 FUSE policy 和 audit。
- `/.skillfs-inbox/<skill>/...` 是 hidden 或 new skill 的安装/修复入口；
  写入落到 source，完成信号可触发外部安全流程。
- `--notify-socket <PATH>` 将 debounce 后的 skill mutation 通知发给外部 daemon。
- `--activation-events-log <PATH>` 将 activation protocol events 写成 JSONL。
- `--activation-reload-mode poll` 在 notify events 后重读 activation state，
  无需 remount 即可更新 resolver。
- startup reconcile 会在挂载启动后对已知 skills 发送 best-effort 通知。
- `--ledger-backing-root <PATH>` 为 in-place activation/notify mount 提供
  daemon 可见的 source 视图，因为公开 source path 已经是 FUSE over-mount。
  推荐使用 `/run/user/$UID/skillfs-ledger/...` 或
  `/run/skillfs-ledger/...` 作为 daemon-facing root。不要使用 `/tmp` 或
  `/var/tmp`：packaged `agent-sec-core.service` 以 `PrivateTmp=true` 运行，
  宿主 tmp 路径对 daemon 不可见，因此会在启动阶段被拒绝。
- `--trusted-writer-exe <PATH>` 是推荐的 mount-path 可信写门禁。它验证
  `/proc/<tgid>/exe`、`(dev, ino)` 和进程 start time，以降低 PID reuse 和
  process-name spoofing 风险。
- `--trusted-writer <NAME>` 是已废弃的兼容门禁，基于 Linux TGID `comm`；
  进程名可伪造，不应用作生产可信依据。
- `--control-socket <PATH>` 配合 `--trusted-peer-exe <PATH>` 启动可信 Unix
  socket control plane。可信 peer 可通过 `meta.writeActivation`、
  `meta.setActivationXattr` 等方法写 activation JSON 或 xattr。

## 文档

- [docs/specs/skillfs-spec.md](docs/specs/skillfs-spec.md) - 架构、运行时一致性边界和部署场景。
- [docs/specs/core-spec.md](docs/specs/core-spec.md) - `skillfs-core` 实现。
- [docs/specs/fuse-spec.md](docs/specs/fuse-spec.md) - `skillfs-fuse` 实现。
- [docs/specs/posix-phase1-spec.md](docs/specs/posix-phase1-spec.md) - POSIX passthrough 基线。
- [docs/testing/posix-phase1-acceptance.md](docs/testing/posix-phase1-acceptance.md) - POSIX 验收清单。
- [docs/testing/posix-external-harness.md](docs/testing/posix-external-harness.md) - external POSIX harness 用法。
- [docs/security/external-decision-protocol.md](docs/security/external-decision-protocol.md) - decision-command JSON 协议。
- [docs/security/runtime-activation-implementation-plan.md](docs/security/runtime-activation-implementation-plan.md) - activation、notify、reload 与 backing-root 集成。
- [docs/skillfs-filesystem-capability-record.md](docs/skillfs-filesystem-capability-record.md) - 长期维护的 filesystem capability record。
- [POSIX_FS_TEST_MATRIX.csv](POSIX_FS_TEST_MATRIX.csv) - POSIX 测试矩阵与当前覆盖。
- [POSIX_FS_REFERENCES.md](POSIX_FS_REFERENCES.md) - POSIX、FUSE 和项目参考资料。

## 验证

这些命令等价于 CI 检查。修改 SkillFS 代码并提交 PR 前应执行。

```bash
# 1. 格式化：必须无 diff
cargo fmt --all --check

# 2. Clippy：在 -D warnings 下必须零 warning
cargo clippy --workspace --all-targets -- -D warnings

# 3. workspace 内单元与集成测试
cargo test --workspace

# 4. 端到端 FUSE mount 测试：需要 fuse3 和 /dev/fuse；在 macOS 或无 /dev/fuse 的容器中会自动跳过
scripts/test.sh

# 5. Rustdoc：修改公共 API 或 doc comments 时必须执行；也有助于尽早发现 intra-doc link 失效
cargo doc --workspace --no-deps
```

注释风格、模块布局、依赖策略、错误处理和 commit 规范见 [AGENTS.md](AGENTS.md)。
