# agent-memory 用户手册（中文）

> English version: [`user_manual.md`](./user_manual.md).

`agent-memory` 是一个仅运行于 Linux 的 Rust [MCP](https://modelcontextprotocol.io/)
服务端，为 AI Agent 提供持久化、沙箱化、以文件为形态的记忆能力。
本手册涵盖架构、安装、配置、25 个 MCP 工具规格、客户端 / SDK 接入指南
以及部署后的功能测试验证流程。

## 目录

1. [简介](#1-简介)
2. [架构设计](#2-架构设计)
3. [安装部署](#3-安装部署)
4. [配置说明](#4-配置说明)
5. [主要功能](#5-主要功能)
6. [接口（Tool API）参考](#6-接口tool-api参考)
7. [SDK / 客户端开发指南](#7-sdk--客户端开发指南)
8. [功能测试与验证](#8-功能测试与验证)
9. [故障排查](#9-故障排查)

---

## 1. 简介

### 什么是 `agent-memory`

`agent-memory` 是一个单二进制 MCP 服务端，把本地文件系统中的一个目录变成
结构化的"记忆仓"，AI Agent 可以通过 19 个明确定义的工具读写它。
与会话上下文窗口或仅向量库的方案不同，这种"记忆"具有：

- **文件形态** —— Agent 以路径思考（`notes/x.md`、
  `decisions/2026-05/db-pick.md`），与人类的文件系统模型一致。
- **沙箱隔离** —— 每次文件 open 都锚定在 mount root，通过
  `openat2(RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS)` 让内核拒绝
  `..`、symlink 以及元目录访问。
- **可版本化** —— 可选的 git 自动 commit + tar.gz 快照分别提供
  文件级 / mount 级回滚。
- **可检索** —— 后台运行 SQLite FTS5 BM25 索引（全文检索次毫秒级），
  可选搭配 OpenAI / Ollama embedding 后端启用稠密向量语义搜索及
  BM25+向量混合检索（RRF 融合排序）。

### 适用场景

- 需要持久化草稿区的 Agent 运行时（Claude Code、Cursor、Continue、
  自研 rmcp 客户端等）。
- 多 Agent 系统中需要 Agent A 写、Agent B 读的笔记跨进程共享。
- 需要审计链路（`mem_log`、JSONL 审计、journald）和恢复手段
  （`mem_revert`、`mem_snapshot_restore`）的运维方。

### 一段话讲清威胁模型

服务端把 Agent 视为不可信进程：可能尝试越界读写、植入 symlink、
批量删除或通过超大 payload 拒绝服务。内核级 `RESOLVE_BENEATH`
+ 显式保留路径集（`.anolisa`、`.git`、`.gitignore`）+ 单次调用大小上限
（`max_read_bytes` / `max_write_bytes` / `max_append_bytes`）封住常见
攻击面。Profile 门控、审计日志和快照属于纵深防御和故障恢复机制。

---

## 2. 架构设计

### 分层结构

```
+--------------------------------------------------------+
| MCP 客户端 (Claude Code / Cursor / 自研)               |
|   stdio JSON-RPC 2.0                                   |
+----------------------------+---------------------------+
                             |
+----------------------------v---------------------------+
| MemoryMcpServer (rmcp)                                 |
|   tools/list -> 按 profile 过滤                        |
|   tools/call -> 入口处再做 profile 校验，返回 Result<>  |
+----------------------------+---------------------------+
                             |
+----------------------------v---------------------------+
| MemoryService                                          |
|   分发到具体 tool 实现；持有 mount / index / git /     |
|   snapshot / audit / session 句柄                      |
+----+------------+--------------+-----------+-----------+
     |            |              |           |
+----v---+   +----v-----+  +-----v----+  +---v-----+
| Mount  |   | Index    |  | Git repo |  | Snapshot|
| (auto/ |   | (SQLite  |  | (libgit2 |  | (tar.gz)|
|  user- |   |  FTS5)   |  |  vendored|  |         |
|  land/ |   |          |  |          |  |         |
|  userns|   |          |  |          |  |         |
+--------+   +----------+  +----------+  +---------+
     |            |              |           |
     +------+-----+--------+-----+-----+-----+
            |              |           |
+-----------v--------------v-----------v----+
| safe_fs: openat2 RESOLVE_BENEATH | NO_SYM |
|          fdopendir + fstatat + unlinkat   |
+--------------------+----------------------+
                     |
+--------------------v----------------------+
| 命名空间 mount: ~/.anolisa/memory/<ns>/   |
|   用户数据 (notes/, decisions/, ...)       |
|   .anolisa/ (audit.log, index.db, ...)    |
+-------------------------------------------+
```

### Mount 策略

| 策略 | 适用 | 行为 |
|------|------|------|
| `userland`（默认） | 任意环境 | mount 仅是普通目录，沙箱由 `openat2` 强制。 |
| `userns` | Linux ≥ 4.6，且内核允许 unprivileged user namespace | 启动时 `unshare` 进入新的 user + mount namespace，在 `/mnt` 上挂一层私有 tmpfs，再把 backing 目录 bind-mount 进去。宿主侧进程看不到 `/mnt/memory/<ns>/` 下的内容。 |
| `auto` | 运行时探测 | 先尝试 `userns`；任何错误均回退到 `userland`。回退路径对部分失败具有鲁棒性（`unshare` / maps 阶段最多执行一次；mount 步骤幂等可重试）。 |

### 命名空间内的目录结构

```
~/.anolisa/memory/user-<uid>/        # mount root
├── README.md                        # 自动生成的概览
├── notes/                           # 自由形态笔记
├── decisions/                       # （示例：用户自定义子目录）
└── .anolisa/                        # OS 管理，Agent 不可写
    ├── manifest.toml                # 命名空间元数据
    ├── audit.log                    # JSONL 工具调用审计
    ├── index.db                     # FTS5 SQLite
    ├── snapshots/                   # tar.gz 归档 + sidecar
    ├── trash/                       # restore 时保留的旧条目
    └── git/                         # bare git 镜像（启用 git 后才有）
```

> 仅为代表性结构 —— `.anolisa/` 下的内容按需懒加载（如 `git/`
> 仅在 `MEMORY_GIT_ENABLED=true` 时存在）。

### 会话目录结构

```
/run/anolisa/sessions/<sid>/         # tmpfs，权限 0700
├── meta.toml                        # 会话元数据
├── log.jsonl                        # 当前会话工具调用日志
└── scratch/                         # 仅会话内的草稿，
                                     # 通过 mem_promote 持久化
```

### 索引 worker

后台 tokio 任务通过 `inotify` 监听 mount，事件经 200 ms debounce 窗口
聚合后，在单个 SQLite 事务中应用。分词器使用 `trigram`（对中文 / 日文友好），
schema 自带版本号便于未来无损迁移。当 inotify 队列溢出
（`IN_Q_OVERFLOW`）时，worker 会自动触发全量 rescan，而不会静默丢事件。

### 审计与可观测性

每次成功的工具调用都会向 `<mount>/.anolisa/audit.log` 追加一行 JSONL，
若启用了会话还会写入 `/run/anolisa/sessions/<sid>/log.jsonl`。
当 `audit.journald=true` 时，每行还会被 fan-out 到 systemd-journald，
带结构化字段（`MESSAGE_ID`、`AGENT_MEMORY_TOOL` 等），便于 `journalctl`
过滤。错误以 MCP 的 `CallToolResult { isError: true }` 形式返回，
让客户端能与"成功但内容包含 'failed' 字面"区分开。

---

## 3. 安装部署

### 通过 RPM 安装（AnolisOS / RHEL 系，推荐）

```bash
sudo yum install agent-memory
```

软件包安装内容：

- `/usr/bin/agent-memory` —— 服务二进制
- `/usr/share/anolisa/agent-memory/default.toml` —— 默认配置
- `/usr/share/anolisa/mcp-servers/agent-memory.json` —— MCP 服务描述符
  （供自动发现）
- `/usr/lib/systemd/user/anolisa-memory@.service` —— 可选的 systemd
  user 模板单元
- `/usr/lib/tmpfiles.d/anolisa-memory.conf` —— 启动时创建
  `/run/anolisa/{,sessions}`（权限 0700）
- `/usr/share/anolisa/adapters/agent-memory/` —— OpenClaw 插件 bundle
  （manifest、源码、预构建 `dist/index.js`、安装脚本）
- `/usr/share/doc/agent-memory/{CHANGELOG.md, user_manual.md, user_manual.zh.md}`

### 安装 OpenClaw 插件（可选）

[OpenClaw](https://github.com/openclaw) 是 Anolis OS 的 Agent 网关，
通过其自有契约消费插件（与裸 MCP stdio 不同）。如果同一台机上还有
通过 `mcp-server.json` 直连 `/usr/bin/agent-memory` 的 MCP 客户端
（Claude Code、Cursor、Continue 等），该客户端会看到全部 19 个原生工具
（`mem_*` + `memory_*`）；本 OpenClaw 插件则独立向 OpenClaw 暴露
4 个 contract 名的子集。两条链路可共存 —— 每个 Agent 只看到所在
runtime 实际广告的工具集。

注册随包附带的插件即可让 4 个 memory contract 工具（`memory_search`、
`memory_get`、`memory_observe`、`memory_get_context`）转发到
agent-memory：

**前置条件**：`openclaw` CLI 必须在 `$PATH` 上。脚本会检测此条件，
缺失时输出明确日志并以 0 退出 —— 安装 OpenClaw 之后重跑即可。

```bash
bash /usr/share/anolisa/adapters/agent-memory/openclaw/scripts/install.sh
openclaw gateway restart
```

OpenClaw 安全扫描器将 `child_process.spawn` 标记为"危险代码模式"，但插件仅用 spawn 启动 agent-memory MCP 服务端作为 stdio 子进程——这是标准 MCP 传输机制而非任意 shell 执行。安装脚本默认绕过扫描器。若需走常规（可能阻断的）安全安装路径，可设 `AGENT_MEMORY_SAFE_INSTALL=1` 调用脚本。

卸载（从 `~/.openclaw/extensions/` 移除插件并清理 `openclaw.json` 的
`plugins.{allow,entries,slots}` 条目）：

```bash
bash /usr/share/anolisa/adapters/agent-memory/openclaw/scripts/uninstall.sh
```

`yum remove agent-memory` 时 spec 的 `%preun` 会自动调用 uninstall
脚本，OpenClaw 配置不会残留孤立插件项。`jq` 优先用于改写
`openclaw.json`；缺失时回退到 `python3`。

插件 contract 名 ↔ agent-memory MCP 工具映射：

| OpenClaw contract | agent-memory MCP 工具 |
|---|---|
| `memory_search` | `memory_search`（Tier B，BM25 默认；配置 embedding 后支持 `mode=vector\|hybrid`） |
| `memory_get` | `mem_read`（Tier A） |
| `memory_observe` | `memory_observe`（Tier B） |
| `memory_get_context` | `memory_get_context`（Tier B） |

插件 MCP `clientInfo.version` 始终与 agent-memory RPM 版本一致 ——
esbuild 在打包时通过 Makefile `sync-versions` target 从 `Cargo.toml`
注入版本号，因此升级 RPM 后 OpenClaw 看到的插件版本会自动跟进。

插件配置（通过 OpenClaw 插件配置 UI 或 `openclaw.json` 的
`plugins.entries["memory-anolisa"].config` 设置）：

| 键 | 类型 | 默认 | 作用 |
|---|---|---|---|
| `binaryPath` | string | 自动发现：先 `$PATH` 中的 `agent-memory`，再依次 `/usr/bin/agent-memory`、`/usr/local/bin/agent-memory`、`~/.local/bin/agent-memory` | agent-memory 二进制绝对路径 |
| `userId` | string | env `USER_ID` → OS `uid`（`process.getuid()`）→ env `$USER` | 命名空间 `user_id`；校验规则与 Rust 侧一致（不含 `..` / `/` / `\` / 控制字符，长度 ≤128 字节） |
| `profile` | `basic` / `advanced` / `expert` | `advanced` | profile 门控（§4）—— 插件以 `MEMORY_PROFILE=<value>` env 启动 `agent-memory serve`，因此 systemd unit 或 shell 中预设的 `MEMORY_PROFILE` **会被插件配置覆盖** |
| `maxReadBytes` | integer (1..4 GiB) | `1048576`（1 MiB） | 单次 `mem_read` 上限；以 `MEMORY_MAX_READ_BYTES` env 传给子进程 |
| `maxWriteBytes` | integer (1..4 GiB) | `16777216`（16 MiB） | 单次 `mem_write` 上限；以 `MEMORY_MAX_WRITE_BYTES` env 传给子进程 |
| `sessionId` | string（`ses_<hex>` 形式） | env `MEMORY_SESSION_ID` → 新生成的 `ses_<random>` 并在 client 生命周期内固定 | 命名空间挂载会话；以 `MEMORY_SESSION_ID` env 传给子进程。一定要固定 —— 若每次 spawn 都不同会导致 `mem_promote` 永远找不到旧 scratch |
| `sessionDir` | string | env `MEMORY_SESSION_DIR` → `/run/anolisa/sessions`（由 `anolisa-memory.conf` tmpfiles 在 boot 时创建） | session scratch + log 根目录；以 `MEMORY_SESSION_DIR` env 传给子进程 |

插件给子进程传递一个最小的 env allowlist（`PATH`、`HOME`、`USER`、
`USER_ID`、`LANG`、`LC_ALL`、`LC_CTYPE`、`TZ`、`TMPDIR`、
`XDG_RUNTIME_DIR`，以及所有以 `MEMORY_` / `RUST_` 起头的变量），
其它来自 OpenClaw 进程的 env 不会泄漏到 agent-memory 子进程。
`USER_ID` 是精确匹配，类似 `USER_IDX` 这种"挂着"前缀的变量不会被放过。

> **兼容性说明**：adapter `manifest.json` 声明
> `compatibleVersions: ">=5.0.0"`。OpenClaw 实际用 CalVer 发布
> （例如 `2026.5.7`），该约束仅作信息提示 —— 插件只用了稳定的
> `openclaw/plugin-sdk` 表面，并在 5.x SDK 形态下验证过。如果未来
> OpenClaw 破坏了 plugin-sdk 契约，应 bump `compatibleVersions`
> 后重新发布。

### 源码构建

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa/src/agent-memory
make build         # cargo build --release --locked
sudo make install  # 安装到 /usr/local 下
```

构建依赖：Rust ≥ 1.85（edition 2024 需 1.85；CI 钉到 1.89.0
是为了与 monorepo 中其他 Linux Rust 子项目用同一镜像 toolchain）、
cmake（libgit2 vendored 构建）、systemd-devel（journald 审计 fan-out 所需）。

### 跨平台开发

`agent-memory` 运行时仅支持 Linux。在 macOS / Windows 上请使用远端
构建流程：

```bash
# 在 src/agent-memory/ 下
make remote-build   # push 分支并 ssh 到 Linux 主机执行 cargo build
make remote-test    # 同上 + 跑测试 + clippy
```

---

## 4. 配置说明

### 配置文件

默认位置：`~/.anolisa/memory.toml`。所有 struct 都启用了
`serde(deny_unknown_fields)`，配置项写错（拼写错误）会在加载时硬失败。
最小配置示例：

```toml
[global]
user_id = "alice"

[memory]
profile = "advanced"           # basic | advanced | expert
max_read_bytes = 1048576       # 1 MiB
max_write_bytes = 16777216     # 16 MiB
max_append_bytes = 4194304     # 4 MiB

[memory.paths]
base_dir = "~/.anolisa/memory"

[memory.session]
base_dir = "/run/anolisa/sessions"
end_action = "discard"         # discard | keep

[memory.mount]
strategy = "auto"              # auto | userland | userns

[memory.index]
enabled = true
time_decay_lambda = 0.01          # 时间衰减系数，0=关闭
time_decay_alpha  = 0.3           # 时间权重占比
cold_after_days   = 30            # 多少天后归档为冷数据
exclude_cold_on_search = true

[memory.audit]
journald = false

[memory.cgroup]
enabled = false
memory_max = "512M"

[memory.git]
enabled = false
auto_commit = true

[memory.consolidation]
enabled                  = true   # 会话结束时自动提取事实
max_facts                = 20     # 每次最多提取
min_tool_calls           = 3      # 最少调用次数门槛
episodic_enabled         = true   # 情景记忆提取
min_episode_steps        = 3      # 情景最少步骤数
max_episodes_per_session = 10
conflict_detection       = true   # 冲突检测
conflict_bm25_threshold  = -2.0   # BM25 冲突阈值
```

### 环境变量覆盖

每个配置项都有对应的 `MEMORY_*` 环境变量，便于测试 / 临时调用：

| 环境变量 | 对应配置 | 说明 |
|----------|----------|------|
| `USER_ID` | `global.user_id` | 经过校验；非法值会被 warn 后忽略。 |
| `MEMORY_BASE_DIR` | `memory.paths.base_dir` | |
| `MEMORY_PROFILE` | `memory.profile` | `basic` / `advanced` / `expert` |
| `MEMORY_SESSION_DIR` | `memory.session.base_dir` | |
| `MEMORY_SESSION_END` | `memory.session.end_action` | |
| `MEMORY_MOUNT_STRATEGY` | `memory.mount.strategy` | |
| `MEMORY_INDEX_ENABLED` | `memory.index.enabled` | systemd 风格的 truthy/falsy |
| `MEMORY_AUDIT_JOURNALD` | `memory.audit.journald` | |
| `MEMORY_CGROUP_ENABLED` | `memory.cgroup.enabled` | |
| `MEMORY_CGROUP_MEMORY_MAX` | `memory.cgroup.memory_max` | `512M` / `2G` / 字节数 |
| `MEMORY_GIT_ENABLED` | `memory.git.enabled` | |
| `MEMORY_GIT_AUTO_COMMIT` | `memory.git.auto_commit` | |
| `MEMORY_MAX_READ_BYTES` | `memory.max_read_bytes` | |
| `MEMORY_MAX_WRITE_BYTES` | `memory.max_write_bytes` | |
| `MEMORY_MAX_APPEND_BYTES` | `memory.max_append_bytes` | |
| `MEMORY_EMBEDDING_BACKEND` | `memory.embedding.backend` | `none` / `openai` / `ollama` |
| `MEMORY_OPENAI_API_KEY` | `memory.embedding.api_key` | 为空时回退到 `OPENAI_API_KEY` |
| `MEMORY_OPENAI_MODEL` | `memory.embedding.model` | 默认 `text-embedding-3-small` |
| `MEMORY_OLLAMA_MODEL` | `memory.embedding.model` | 默认 `nomic-embed-text` |
| `MEMORY_OLLAMA_BASE_URL` | `memory.embedding.base_url` | 默认 `http://localhost:11434` |
| `MEMORY_INDEX_TIME_DECAY_LAMBDA` | `memory.index.time_decay_lambda` | 需 ≥ 0.0 |
| `MEMORY_INDEX_TIME_DECAY_ALPHA` | `memory.index.time_decay_alpha` | 需 0.0–1.0 |
| `MEMORY_INDEX_COLD_AFTER_DAYS` | `memory.index.cold_after_days` | |
| `MEMORY_INDEX_EXCLUDE_COLD` | `memory.index.exclude_cold_on_search` | |
| `MEMORY_CONSOLIDATION_ENABLED` | `memory.consolidation.enabled` | |
| `MEMORY_CONSOLIDATION_MAX_FACTS` | `memory.consolidation.max_facts` | |
| `MEMORY_CONSOLIDATION_MIN_CALLS` | `memory.consolidation.min_tool_calls` | |
| `MEMORY_EPISODIC_ENABLED` | `memory.consolidation.episodic_enabled` | |
| `MEMORY_MIN_EPISODE_STEPS` | `memory.consolidation.min_episode_steps` | |
| `MEMORY_MAX_EPISODES` | `memory.consolidation.max_episodes_per_session` | |
| `MEMORY_CONFLICT_DETECTION` | `memory.consolidation.conflict_detection` | |
| `MEMORY_CONFLICT_THRESHOLD` | `memory.consolidation.conflict_bm25_threshold` | |
| `MEMORY_SESSION_ID` | （仅运行时） | 把当前 Agent 运行固定到 `MEMORY_SESSION_DIR` 下指定的 session id；`mem_promote` 必须设置，详见 §7。 |

### Profile 含义

Profile 是 UX 提示而非安全边界，但在 `tools/list` 和 `tools/call`
两层都做了校验：

- **basic** —— 25 个工具全部展示；弱模型也能用 Tier B 的结构化 API。
- **advanced**（默认） —— 25 个工具全部展示；强模型应优先使用 Tier A
  文件操作。
- **expert** —— 隐藏 Tier B（`memory_search`、`memory_observe`、
  `memory_get_context`），且 `tools/call` 调用会以 `METHOD_NOT_FOUND`
  拒绝。已经熟练操作文件系统的前沿模型只需要 Tier A 与 Tier C。


### Embedding / 语义搜索

配置 `[memory.embedding]` 段启用稠密向量语义搜索：

```toml
[memory.embedding]
backend = "openai"
api_key = ""           # 为空时自动读 OPENAI_API_KEY 环境变量
model = "text-embedding-3-small"
```

或使用本地 Ollama（无需 API key）：

```toml
[memory.embedding]
backend = "ollama"
model = "nomic-embed-text"
base_url = "http://localhost:11434"
```

启用后，`memory_search` 支持 `mode` 参数：
- `mode="bm25"`（默认）—— 纯关键词搜索，向后兼容
- `mode="vector"` —— 纯语义搜索（余弦相似度）
- `mode="hybrid"` —— RRF (k=60) 融合 BM25 + 向量排序

未配置 embedding 时，`vector` / `hybrid` 模式会自动降级为 BM25，不会报错。
---

## 5. 主要功能

### Tier A —— 文件操作（11 个工具）

`mem_read` / `mem_write` / `mem_append` / `mem_edit` / `mem_list` /
`mem_grep` / `mem_diff` / `mem_mkdir` / `mem_remove` / `mem_promote` /
`mem_session_log`。

Agent 以 mount 相对路径思考。保留前缀（`.anolisa`、`.git`、`.gitignore`）
在写入时被拒绝。`mem_edit` 要求 `old_str` 恰好命中一次（0 次或多次都
报错），避免悄悄改错位置。`mem_promote` 把会话 `scratch/` 中的文件原子
移入持久化仓。

### Tier B —— 结构化检索（3 个工具）

`memory_search` 在 FTS5 索引上跑 BM25 关键字查询（默认），返回排序片段。
当配置了 embedding 后端（OpenAI 或 Ollama）后，`mode="vector"` 启用
纯语义搜索，`mode="hybrid"` 则以 RRF（Reciprocal Rank Fusion, k=60）
融合 BM25 和向量结果，兼顾关键词匹配与语义理解。无 embedding 时
vector/hybrid 会自动降级为 BM25，不会报错。

`memory_observe` 把一段内容连同 frontmatter 写到
`notes/observed/<ULID>.md`，让 Agent "零决策"地记下一个想法。
`memory_get_context` 按 token 上限拼出最近修改文件的 markdown 预览，
适合在每次回合开始时让 Agent "看一眼仓里都有什么"。返回的每条结果
含 `suspicious` 字段，标记是否命中注入检测模式。

### Tier C —— 治理（5 个工具）

`mem_snapshot` / `mem_snapshot_list` / `mem_snapshot_restore` 提供
mount 范围的时间点备份（tar.gz + sidecar 元数据）。`mem_log` 与
`mem_revert` 操作可选的 git 镜像 —— 适用于 "我三回合前改错文件了" 这种
回滚需求。

### 沙箱保证

- 路径穿越（`..`、绝对路径、`\0`） → 内核通过 `openat2` 拒绝。
- 调用中途的 symlink 替换 → 由 `RESOLVE_NO_SYMLINKS` 内核级阻止；
  递归删除使用 `fdopendir` + `fstatat(AT_SYMLINK_NOFOLLOW)` +
  `unlinkat`，让 swap 无法 race。
- 写覆盖保留路径（`.anolisa/audit.log`、`.gitignore` 等） →
  由 `TargetIsReserved` 拒绝。
- payload 超大 → 按 `max_*_bytes` 配置拒绝。
- `mem_snapshot_restore` 中混入 symlink → tar entry-type 过滤
  拒绝 `Symlink` / `Hardlink` / `Device` / `Fifo`。

---

## 6. 接口（Tool API）参考

所有工具都通过 MCP `tools/call` 调用，参数为 JSON 对象。错误以
`CallToolResult { isError: true, content: [{type: "text", text:
"<原因>"}] }` 形式返回，客户端可据此分支处理。

### Tier A

| 工具 | 必填 | 可选 | 返回 |
|------|------|------|------|
| `mem_read` | `path` | — | UTF-8 文件内容 |
| `mem_write` | `path`、`content` | `overwrite` | `wrote N bytes to <path>` |
| `mem_append` | `path`、`content` | — | `appended N bytes to <path>` |
| `mem_edit` | `path`、`old_str`、`new_str` | — | `edited <path>` |
| `mem_list` | — | `dir`、`recursive`、`glob` | `{name, type, size, mtime}` 数组 |
| `mem_grep` | `pattern` | `dir`、`type`、`max`、`case_insensitive` | `{path, line, text}` 数组 |
| `mem_diff` | `path1`、`path2` | — | unified diff |
| `mem_mkdir` | `path` | — | `created <path>` |
| `mem_remove` | `path` | `recursive` | `removed <path>` |
| `mem_promote` | `session_path`、`store_path` | — | `promoted N bytes: <src> -> <dst>` |
| `mem_session_log` | — | — | 会话 JSONL 或 `(session log is empty)` |

### Tier B

| 工具 | 必填 | 可选 | 返回 |
|------|------|------|------|
| `memory_search` | `query` | `top_k`（默认 5）、`mode`（bm25/vector/hybrid）、`category`（lesson/interest/…） | `{path, score, snippet, suspicious}` 数组 |
| `memory_observe` | `content` | `hint` | `observed at notes/observed/<ulid>.md` |
| `memory_get_context` | — | `max_tokens`（默认 2048） | markdown 预览 |

### Tier C

| 工具 | 必填 | 可选 | 返回 |
|------|------|------|------|
| `mem_snapshot` | — | `name` | JSON `{id, name, created_at, size, backend}` |
| `mem_snapshot_list` | — | — | 按 created_at 升序的数组 |
| `mem_snapshot_restore` | `id` | — | `restored <id>` |
| `mem_log` | — | `limit`（默认 20）、`path` | `{hash, summary, author, time}` 数组 |
| `mem_revert` | `path` | — | `reverted <path> (commit <hash>)` |
| `mem_consolidate` | — | — | `consolidation complete: N facts written` |
| `mem_compact` | — | — | `compacted N files to cold storage` |

### 错误码语义

| MCP 错误码 | 含义 |
|------------|------|
| `-32601` METHOD_NOT_FOUND | 当前 profile 隐藏了该工具 |
| `-32602` INVALID_PARAMS | 缺参或类型错 |
| `-32603` INTERNAL_ERROR | 服务端故障 |
| `isError: true` | 工具运行了但返回了业务错误（路径不存在、被沙箱拒绝、大小超限等） |

---

## 7. SDK / 客户端开发指南

### 接入 MCP 兼容客户端

#### Claude Code（`.claude/settings.json`）

```json
{
  "mcpServers": {
    "agent-memory": {
      "command": "/usr/bin/agent-memory",
      "args": [],
      "env": {
        "USER_ID": "alice",
        "MEMORY_PROFILE": "advanced"
      }
    }
  }
}
```

#### Cursor / Continue / 任意 stdio MCP 客户端

按相同的 `command` / `args` / `env` 形态指向二进制即可。
`/usr/share/anolisa/mcp-servers/agent-memory.json` 描述符列出了
全部 25 个工具名，支持自动发现的客户端能直接识别。

### 程序化接入

#### Python（官方 `mcp` SDK）

```python
import asyncio
from mcp import ClientSession, StdioServerParameters
from mcp.client.stdio import stdio_client

async def main():
    server = StdioServerParameters(
        command="/usr/bin/agent-memory",
        args=[],
        env={"USER_ID": "alice"},
    )
    async with stdio_client(server) as (read, write):
        async with ClientSession(read, write) as session:
            await session.initialize()
            tools = await session.list_tools()
            print([t.name for t in tools.tools])

            result = await session.call_tool(
                "mem_write",
                {"path": "notes/from-python.md", "content": "hello"},
            )
            assert not result.isError
            print(result.content[0].text)

asyncio.run(main())
```

#### TypeScript（`@modelcontextprotocol/sdk`）

```typescript
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const transport = new StdioClientTransport({
  command: "/usr/bin/agent-memory",
  args: [],
  env: { USER_ID: "alice" },
});
const client = new Client({ name: "my-app", version: "1.0.0" }, {});
await client.connect(transport);

const result = await client.callTool({
  name: "mem_grep",
  arguments: { pattern: "TODO", recursive: true, max: 50 },
});
console.log(result.isError ? "failed" : result.content);
```

#### Rust（`rmcp`）

```rust
use rmcp::transport::child_process::ChildProcessTransport;
use rmcp::ServiceExt;

let transport = ChildProcessTransport::new(
    tokio::process::Command::new("/usr/bin/agent-memory"),
).await?;
let client = ().serve(transport).await?;
let tools = client.list_tools(Default::default()).await?;
let resp = client.call_tool(rmcp::model::CallToolRequestParam {
    name: "mem_read".into(),
    arguments: Some(serde_json::json!({"path": "notes/x.md"})
        .as_object().unwrap().clone()),
}).await?;
```

### Promote 工作流接入（多回合模式）

适用于"先草稿、决定后才持久化"的 Agent：

1. 每次 Agent 运行设置 `MEMORY_SESSION_ID=<sid>` 和
   `MEMORY_SESSION_DIR=/run/anolisa/sessions`。
2. Agent 把草稿写到 `/run/anolisa/sessions/<sid>/scratch/` 下
   （由运行时把文件落到 scratch 目录）。
3. Agent 决定"这条值得保留"时，调用 `mem_promote` 原子地把文件
   移入持久化仓。

### 可观测性接入点

- `audit.journald=true` —— 每次调用 fan-out 到
  `journalctl --user-unit=anolisa-memory@<user>`。
- `mem_session_log` —— Agent 自身可读取本回合的 JSONL，做"自我反思"。
- `mem_log`（启用 git 后） —— 把变更历史暴露给 Agent；配合
  `mem_revert` 给 Agent 一个真正的"撤销"按钮。

---

## 8. 功能测试与验证

### 8.1 自动化测试

```bash
cd src/agent-memory
cargo fmt --check
cargo clippy -- -D warnings
cargo test                                        # 全部 suite
cargo test --test e2e_agent_test                  # 19 工具 E2E
cargo test --test mcp_integration_test            # 协议层
cargo test --test linux_userns_test -- --ignored  # 需要 unprivileged userns
```

`ci.yaml` 上的 CI Job 会跑 `fmt --check` + `clippy -D warnings` +
`cargo test`，Rust 版本锁定 1.89。

### 8.2 交互式 `mcp-harness`

`mcp-harness` 是一个 example 二进制，通过 stdio 驱动服务端，并提供
REPL 用于手动调用工具：

```bash
cargo run --example mcp-harness -- /tmp/mem-test
```

| 命令 | 说明 |
|------|------|
| `list` | 列出当前可见工具 |
| `call <tool> <json_args>` | 调用某个工具 |
| `help` | 显示命令帮助 |
| `quit` | 关闭服务并退出 |

示例会话：

```
mcp> call mem_mkdir {"path": "notes"}
Result: created notes
mcp> call mem_write {"path": "notes/day1.md", "content": "Hello world"}
Result: wrote 11 bytes to notes/day1.md
mcp> call mem_read {"path": "notes/day1.md"}
Result: Hello world
```

预置场景（无断言，由人观察输出确认）：

```bash
cargo run --example mcp-harness -- /tmp/mem-test --scenario full
cargo run --example mcp-harness -- /tmp/mem-test --scenario git --git
cargo run --example mcp-harness -- /tmp/mem-test --scenario promote
cargo run --example mcp-harness -- /tmp/mem-test --verbose   # 打印 JSON-RPC
```

### 8.3 直发 JSON-RPC（协议级调试）

启动服务并向其 stdin 喂 JSON-RPC：

```bash
mkdir -p /tmp/mem-test/__sessions__
MEMORY_BASE_DIR=/tmp/mem-test \
MEMORY_SESSION_DIR=/tmp/mem-test/__sessions__ \
MEMORY_MOUNT_STRATEGY=userland \
USER_ID=tester \
agent-memory
```

握手：

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"manual","version":"1.0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
```

工具调用：

```json
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mem_write","arguments":{"path":"test.md","content":"hello"}}}
```

预期响应：

```json
{"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"wrote 5 bytes to test.md"}],"isError":false}}
```

### 8.4 沙箱越界验证

确认内核沙箱拒绝以下逃逸：

```json
{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"mem_read","arguments":{"path":"../../etc/passwd"}}}
```
→ `isError: true`，消息 `path outside mount root`。

```json
{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"mem_write","arguments":{"path":".anolisa/audit.log","content":"x"}}}
```
→ `isError: true`，消息 `target is reserved`。

```json
{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"mem_read","arguments":{"path":"a/b/symlink-to-etc-passwd"}}}
```
→ `isError: true`，消息 `path outside mount root`（内核 ELOOP）。

### 8.5 单工具验证流程

下列流程既可在 harness REPL 中（`call <tool> <json>`）执行，也可
通过 raw JSON-RPC 验证。在 `mcp-harness` 中跑最直接：

- **mem_mkdir** —— `call mem_mkdir {"path":"d"}`，响应包含 `created`；
  再 `call mem_list {"recursive": true}` 验证。
- **mem_write / mem_read** —— 写入 `Hello world\n`，读回字节级一致；
  `overwrite=false` 重写应返回错误。
- **mem_append** —— 追加 `+more`，再读，内容等于 `原文+more`。
- **mem_edit** —— 写入 `foo bar baz`，把 `bar` 编辑为 `qux`，
  读回得 `foo qux baz`；再次执行（`bar` 已不存在）应报错
  `match count 0`。
- **mem_list** —— 创建嵌套目录与文件，递归列表应包含全部路径，
  外加 init 时自动产生的 `README.md`。
- **mem_grep** —— 写入两个含不同关键词的文件，搜索其中一个关键词
  应只命中匹配文件，且每个 hit 含 `path / line / text`。
- **mem_diff** —— 对两个文件 diff，输出以 `--- ` / `+++ ` 行起始
  的 unified diff。
- **mem_remove** —— 删除文件后再读应报错 `not found`。
- **mem_promote** —— 预创建 `MEMORY_SESSION_DIR/<sid>/scratch/x.md`，
  设置环境变量后调用 promote，再读目标路径。
- **mem_session_log** —— 任意调用 3 次工具后 `mem_session_log` 应返
  回 3 行 JSONL。
- **memory_observe** —— 调用两次 observe；递归列 `notes/observed`
  应有两个 ULID 命名的文件。
- **memory_search** —— 用关键字 `kappa` observe，等待 ~500 ms，再
  search `kappa`，结果包含该 observe 文件。
- **memory_get_context** —— 写入 5 个有不同首行的文件，
  `memory_get_context {max_tokens: 200}` 返回的预览能见到它们。
- **mem_snapshot / list** —— 创建快照后 list 应有条目；
  size > 0；id 以 `snap_` 起头。
- **mem_snapshot_restore** —— 写 v1，快照，写 v2，restore 快照后
  读回得 v1；`.anolisa/trash/<ts>-<id>/` 中保留有 v2。
- **mem_log** —— 启用 git 后写入同一文件三个版本，
  `mem_log {path: "..."}` 至少返回 3 条 commit。
- **mem_revert** —— 启用 git 后，写 v3，revert，再读得最近一次提交
  内容（v2）。

### 8.6 一键冒烟测试

Makefile 自带一个独立 smoke target，会驱动 5 个工具走完整流程并
校验响应：

```bash
cd src/agent-memory
make smoke
```

看到绿色的 `==> Smoke test PASSED` 即可认为部署端到端正常。

---

## 9. 故障排查

| 症状 | 可能原因 | 处理 |
|------|----------|------|
| 启动报 `unshare(NEWUSER\|NEWNS): EPERM` | unprivileged user namespace 被禁 | `sysctl kernel.unprivileged_userns_clone=1`，或者改 `MEMORY_MOUNT_STRATEGY=userland`。 |
| `tmpfs /mnt: EBUSY` | 新 namespace 中 `/mnt` 已被其他 mount 占据 | 重试逻辑会把 EBUSY 视作成功；如仍持续，重启进程。 |
| macOS / Windows 上 `cargo build` 报 `libsystemd` / `nix` 错 | 宿主非 Linux | 改用 `make remote-build` / `remote-test`。 |
| `tools/call memory_search` 返 `METHOD_NOT_FOUND` | `MEMORY_PROFILE=expert` 隐藏了 Tier B | 切回 `advanced`，或直接用 Tier A 文件工具。 |
| 配置项 typo 被悄悄忽略 | 旧版本会 default-fill 错字段 | 现已硬失败：看启动 stderr 的报错并修正。 |
| `mem_log` 返回 `[]` 即使有写入 | git 版本控制未启用 | `MEMORY_GIT_ENABLED=true MEMORY_GIT_AUTO_COMMIT=true`。 |
| 索引检索对刚写入的内容查不到 | 还在 200 ms debounce 窗口内 | 重试；或改用 `mem_grep`（直接走文件系统正则，不依赖索引）。 |
| `mem_promote` 报 `session not found` | `MEMORY_SESSION_ID` / `MEMORY_SESSION_DIR` 未设或 scratch 不存在 | 见 §7 Promote 工作流接入。 |

更深入的排查：用 `RUST_LOG=agent_memory=debug` 启动，同时检查
服务端 stderr 与 `<mount>/.anolisa/audit.log`。

---

## 许可证

Apache-2.0。详见随包发布的 `LICENSE`。

## 反馈问题

[`github.com/alibaba/anolisa/issues`](https://github.com/alibaba/anolisa/issues)，
组件 `memory`。
