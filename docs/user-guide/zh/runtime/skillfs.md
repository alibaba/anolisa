# SkillFS

SkillFS 是面向 Agent Skill 的 FUSE 虚拟文件系统。它把物理 skill 源目录映射成
稳定的运行时视图，读取 `SKILL.md` 时返回编译后内容，普通文件仍由底层 source
树承载。

SkillFS 不做业务层安全判断。agent-sec-core 或 Skill Ledger 等外部组件负责扫描
skill 并写入 activation 状态。SkillFS 消费这些状态，将每个 skill 暴露为 live、
fallback snapshot 或 hidden。

## 适用场景

适合使用 SkillFS 的场景：

- 需要给 Agent 提供稳定的挂载路径；
- 需要隔离 source workspace 和 Agent 可见视图；
- 需要 default view 过滤，并通过 `skill-discover` 发现 secondary skills；
- 生产访问需要 in-place policy 和 audit 覆盖；
- 需要对接 Skill Ledger，提供 fallback / hidden 运行时视图；
- 需要保护 `.skill-meta`，避免普通 Agent 进程读取元数据。

不要直接对已有 hub workspace 做 in-place mount，尤其是该 workspace 同时包含
`.hub` 目录、外部 manifest 或 registry 元数据时。推荐分离 hub workspace 和
干净的 SkillFS source root。

## 环境要求

| 条件 | 说明 |
| --- | --- |
| OS | FUSE mount 需要 Linux |
| FUSE | FUSE3（`libfuse3-dev`、`fuse3` 或等价包） |
| 设备 | `/dev/fuse` 必须可用 |
| Rust | 源码构建需要 1.86+ |

macOS 可以运行 `validate`、`list`、`classify` 等不依赖 FUSE 的命令，但不能挂载
SkillFS。

## 安装

```bash
# 推荐包安装
anolisa install skillfs

# 开发者源码构建
cd src/skillfs
cargo +1.86.0 build --release
```

## Source 布局

SkillFS 期望 source 目录下每个子目录对应一个 skill：

```text
/path/to/skills/
  demo-weather/
    SKILL.md
    scripts/
      run.sh
  demo-search/
    SKILL.md
    config.json
```

目录名是权威运行时 skill id。`SKILL.md` frontmatter 里的 `name` 字段是展示元数据，
不会覆盖目录 key。

不要把 `.skill-meta` 当作普通 Agent 数据使用。它保存 SkillFS 和 ledger 元数据，
对普通调用方隐藏。

## 快速开始

```bash
# 验证源目录中的 skills
skillfs validate /path/to/skills

# 列出所有 skills
skillfs list /path/to/skills

# 生成 skillfs-views.toml
skillfs classify /path/to/skills

# 挂载虚拟文件系统
skillfs mount /path/to/skills /mnt/skillfs --foreground
```

normal mount 后，Agent 读取：

```text
/mnt/skillfs/skills/<skill-name>/SKILL.md
```

前台测试挂载可以用 `Ctrl+C` 停止，也可以执行：

```bash
fusermount3 -u /mnt/skillfs
```

## 挂载布局

### Normal Mount

Normal mount 使用不同的 source 和 mountpoint 目录：

```bash
skillfs mount /path/to/skills /mnt/skillfs --foreground
```

Agent 通过 `<MOUNTPOINT>/skills` 访问 skill。直接写 source 目录会绕过 SkillFS
policy 和 audit；通过挂载路径写入时会透传到底层 source 树。

适合本地开发、兼容性检查，以及 source workspace 由其他进程管理的环境。

### In-place Mount

In-place mount 使用同一个目录作为 source 和 mountpoint：

```bash
skillfs mount /path/to/skills /path/to/skills \
  --foreground \
  --security-mode \
  --audit-log /var/log/skillfs/audit.jsonl
```

SkillFS 会 over-mount source 目录，因此普通用户态访问会经过 FUSE policy 和
audit。In-place mount 不额外增加 `/skills` 层：

```text
/path/to/skills/<skill-name>/SKILL.md
```

生产安全集成建议使用 in-place mount。会替换或 rename mountpoint 目录本身的工具，
例如 workspace checkpoint 或 rollback 工具，必须在 mount 前或 unmount 后运行。

### Managed Mount

`--managed` 会启动 detached supervisor，将 desired state 保持为 mounted，并在
worker 意外退出后重新挂载：

```bash
skillfs mount /path/to/skills /mnt/skillfs --managed
skillfs stop /mnt/skillfs
```

`skillfs stop <MOUNTPOINT>` 会清除 desired state、终止 supervisor 和 worker，并
执行 unmount。该命令是幂等的，挂载已经停止时重复执行也安全。

Managed mode 还会在 worker 异常退出后检测 stale/dead FUSE endpoint，清理后按
有界重试重新挂载。默认 foreground mount 行为不变：收到 `SIGTERM` 或 `Ctrl+C`
时仍会退出并 unmount。

## CLI 工具

### validate

```bash
skillfs validate /path/to/skills
skillfs validate /path/to/skills --format json
```

`validate` 会报告 successful、degraded 和 failed skill parse。Parse failure 会
进入状态汇总并返回非 0 exit code；仅 degraded 的 skill 会被报告，但 exit code 仍为
0。

JSON 输出中，error 和 warning entry 都包含 `path` 字段，方便调用方定位具体出错的
skill 文件。

### list 和 classify

```bash
skillfs list /path/to/skills
skillfs list /path/to/skills --enabled-only
skillfs classify /path/to/skills --primary-count 6
skillfs classify /path/to/skills --dry-run
```

`list` 输出发现的 skills 及其元数据。`classify` 生成或预览
`skillfs-views.toml`；前 N 个 skills 进入 default view，其余进入 secondary view。

## Views 与 Discovery

source 目录下的 `skillfs-views.toml` 控制可见性：

```toml
[[view]]
name = "major"
default = true
description = "Core skills shown directly in /skills"
skills = ["github", "notion", "slack"]

[[view]]
name = "other"
default = false
description = "Additional skills accessible through skill-discover"
skills = ["apple-notes", "blogwatcher"]
```

default view 会直接出现在挂载后的 skill 视图中。Secondary views 由虚拟
`skill-discover` skill 列出，其 `SKILL.md` 包含 skill 名称和 source path。

未分配到任何 view 的 skill 会在下次 mount 时加入 default view。

## 读写语义

| 操作 | 行为 |
| --- | --- |
| `readdir` | 由 views 和 runtime activation state 控制 |
| 读取 `SKILL.md` | 返回编译后内容，不是原始 source 文本 |
| 读取普通文件 | 透传到底层物理 source 树 |
| 写入 `SKILL.md` | 写透传，并重新解析 store |
| 写入普通文件 | 写透传，不改变 skill 元数据 |
| rename skill 目录 | 目录名是权威 key |
| symlink 或 hardlink | 限制为安全的同 skill 相对目标 |
| `user.*` xattr | 普通路径上保守透传 |

In-place authoring 支持新建 skill 目录。刚创建但尚未写入 manifest 的目录不会暴露
phantom `SKILL.md`；写入 `SKILL.md` 后，SkillFS 会重新解析并暴露编译后的视图。
Pending 或 direct-final install 可以保留普通顶层 skill 目录的 mode、timestamp 和
ownership 等元数据。`.skill-meta/**` 仍只允许 trusted metadata path 访问。

无安全集成时，skill 默认读取 live source 树。启用 security activation 后，
可见性受 active mapping 限制：

- current：读取 live source 树，例如旧 decision-command resolve 路径产生的状态；
- fallback：读取 `.skill-meta` 下的可信 snapshot；
- hidden：对普通调用方隐藏 skill。

Activation file mode 下，Activation JSON 表达 fallback 和 hidden 两种状态，不写
current/live 状态。如果该模式下某个 skill 没有 activation JSON 或 activation
xattr，SkillFS 会按 fail-safe 默认值将其视为 hidden。

## 安全集成

### Activation File Mode

当外部 daemon 接收 SkillFS mutation event、扫描 source 树并写入 activation 元数据
时，使用 activation file mode：

```bash
skillfs mount /path/to/skills /mnt/skillfs \
  --foreground \
  --security \
  --activation-mode file \
  --notify-socket /run/skill-ledger.sock \
  --activation-events-log /var/log/skillfs/activation-events.jsonl \
  --activation-reload-mode poll
```

流程：

```text
Agent 或 installer 通过 SkillFS 写入
  -> SkillFS 发送 notify event
  -> Skill Ledger 扫描并写 activation state
  -> SkillFS reload activation state
  -> skill 变为 live、fallback 或 hidden
```

`--activation-reload-mode poll` 需要 `--notify-socket` 或
`--activation-events-log`，因为 SkillFS 需要触发源来启动 polling。

对于 in-place activation 和 notify mount，需要设置 `--ledger-backing-root`，给
daemon 一个可见的 backing source path：

```bash
skillfs mount /path/to/skills /path/to/skills \
  --security-mode \
  --security \
  --activation-mode file \
  --notify-socket /run/skill-ledger.sock \
  --ledger-backing-root /run/user/$UID/skillfs-ledger/source
```

当 daemon 使用 `PrivateTmp=true` 运行时，避免把集成路径放在 `/tmp` 或
`/var/tmp`；这些路径对 daemon 不可见，并会被启动校验拒绝。

### Control Socket

可信 control socket 是生产环境推荐的 activation 写入路径：

```bash
skillfs mount /path/to/skills /mnt/skillfs \
  --security \
  --activation-mode file \
  --control-socket /run/skillfs/control.sock \
  --trusted-peer-exe /usr/bin/skill-ledger
```

该 socket 要求 `--security --activation-mode file`，与 `--decision-command` 互斥，
并且必须指定可信 peer executable。Peer 校验使用 Linux peer credential 和
executable identity。

支持的 JSONL 请求示例：

```json
{"schemaVersion":"1","method":"ping"}
{"schemaVersion":"1","method":"status"}
{"schemaVersion":"1","method":"meta.writeActivation","skillName":"demo-weather","activation":{"schemaVersion":1,"target":null}}
{"schemaVersion":"1","method":"meta.setActivationXattr","skillName":"demo-weather","activation":{"schemaVersion":1,"target":null}}
```

### Trusted Mount-path Writer

`--trusted-writer-exe <PATH>` 是 mount path 可信写入兼容门禁。新生产集成优先使用
control socket。

`--trusted-writer <NAME>` 已废弃，仅匹配 Linux 进程 `comm` 名称。兼容条件允许时，
应使用 executable identity。

### Decision-command Mode

`--security --decision-command <COMMAND>` 是旧的兼容路径。SkillFS 调用外部命令执行
scan 和 resolve 决策。

Decision-command mode 与 activation file mode、`--notify-socket`、
`--activation-events-log`、`--ledger-backing-root` 和 `--control-socket` 互斥。

## Install 协议

SkillFS 支持面向 installer 的生命周期路径：

- staging roots 可从普通 listing 隐藏，但精确 staging 路径仍可写；
- direct-to-final install 可在 activation 出现前保持 hidden；
- `/.skillfs-inbox/<skill>/...` 是 hidden 或 new skill 的 install/repair 入口；
  写入会落到 source tree，并可触发外部安全流程；
- quiet-timeout notification 可在配置的 quiet window 后聚合 install mutation；
- post-publish grace 可允许发布后有界的 installer 元数据写入；
- fallback skill 的 post-publish grace 路径会路由到 live source，便于 installer
  在 publish 后完成元数据更新。

这些行为通过 SkillFS TOML config 配置，并需要 `--notify-socket` 或
`--activation-events-log` 等 notify source。

## 可观测性

### Audit 和 Activation Logs

`--audit-log <PATH>` 写 filesystem audit events JSONL。
`--activation-events-log <PATH>` 写 daemon-driven activation 流程中的 activation
protocol events JSONL。

### SLS Ops 和 Runtime Metrics

SkillFS 会 best-effort 写 SLS records 到：

```text
/var/log/anolisa/sls/ops/skillfs.jsonl
```

该文件由部署侧/SLS 组件拥有并预创建。SkillFS 只在文件存在时追加写入；不会创建该文件
或父目录，写入失败也不会改变 CLI 或 FUSE 行为。

以下 CLI 命令会追加 ops records：`mount`、`list`、`validate` 和 `classify`。mount
存活期间，runtime metric record 使用 `record_type = "runtime_metric"`，覆盖 mount
lifecycle、view pruning、skill hits 和 security policy outcomes。兼容旧消费者的
mount-session summary 仍共享同一个文件。

## 常用参数

| 参数 | 作用 |
| --- | --- |
| `--foreground` | 前台运行 |
| `--managed` | 启动 detached supervised mount |
| `--security-mode` | 要求 source 和 mountpoint 是同一路径 |
| `--security` | 启用安全集成 |
| `--activation-mode file` | 消费 activation JSON/xattr 状态 |
| `--activation-reload-mode poll` | notify trigger 后 poll activation |
| `--notify-socket <PATH>` | 向外部 daemon 发送 mutation event |
| `--activation-events-log <PATH>` | 写 activation protocol events JSONL |
| `--audit-log <PATH>` | 写 filesystem audit events JSONL |
| `--control-socket <PATH>` | 接收可信 activation 写请求 |
| `--trusted-peer-exe <PATH>` | 固定可信 control socket peer |
| `--trusted-writer-exe <PATH>` | 固定可信 mount-path writer |
| `--ledger-backing-root <PATH>` | 提供 daemon 可见的 source view |
| `--decision-command <CMD>` | 使用旧 external decision 模式 |
| `--pid-file <PATH>` | 写进程 pid file |
| `--allow-other` | 允许其他用户访问 FUSE mount |
| `--config <PATH>` | 加载 SkillFS TOML 配置 |
| `-v`, `--verbose` | 启用 debug logging |
| `--log-file <PATH>` | 将日志写入文件 |

## 排障

**新安装的 skill 不可见。**
启用 security activation 后，新 skill 可能保持 hidden，直到 ledger 写入 activation
state。检查 notify 投递和 activation reload events。

**Fallback 读到旧版本。**
这是预期行为。Fallback 读取 `.skill-meta` 下的可信 snapshot，而不是 live source。

**看不到 `.skill-meta`。**
普通调用方看不到该目录是预期行为。可信 peer 可以通过配置的 trusted path 访问元数据。

**日志里出现 notify socket 失败。**
Notify 失败只是 warning，不会停止 FUSE 服务，但外部 daemon 可能收不到 mutation
event，直到 socket 修复。

**In-place activation 启动失败。**
检查是否设置了 `--ledger-backing-root`，以及该路径对 daemon 可见。使用
`PrivateTmp=true` 的服务不要使用 `/tmp` 或 `/var/tmp`。

**Managed mount 在 launcher 重启后仍然存在。**
这是预期行为。使用 `skillfs stop <MOUNTPOINT>` 停止。

## 更多参考

- [SkillFS README](../../../../src/skillfs/README_zh.md)
- [External decision protocol](../../../../src/skillfs/docs/security/external-decision-protocol.md)
- [Runtime activation plan](../../../../src/skillfs/docs/security/runtime-activation-implementation-plan.md)
- [FUSE crate layout](../../../../src/skillfs/docs/architecture/fuse-crate-layout.md)
