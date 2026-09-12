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

全局配置文件的读取方是 daemon，因此 `config -g` 不止于写文件：写入 `/etc/ws-ckpt/config.toml` 后，它会请求 daemon 重载，并把 daemon 实际加载到的配置与刚写入的逐项比对。只要有任何不一致，命令会列出每个差异字段并以非零退出，而不是报成功。

这一点在 Kubernetes sidecar 部署中尤其重要：CLI（app 容器）与 daemon 运行在不同容器、各自独立的文件系统里。需把 `/etc/ws-ckpt` 挂到两容器共享的卷上（`emptyDir` 即可）；否则每一条 `config -g` 设置都会静默停留在 daemon 的内置默认值。随附的 `k8s-sidecar-example.yaml` 已经接好了这个共享卷。

---

## 重要注意事项

> **警告**：ws-ckpt 配置的工作区路径**不能**是：
> - 根路径（`/`）
> - daemon mount_path 内部的路径
> - 活跃的挂载点（见下文）
> - Agent 启动目录或其父目录（在 plugin 层校验）
>
> 这些约束由 daemon 代码强制执行。使用无效路径将被拒绝。

### 工作区根目录不能是挂载点

初始化工作区时会把原目录改名后作为备份，而 `rename(2)` 对「自身是挂载点」的目录会返回
`EBUSY`。这与文件系统类型无关，不只是 FUSE。

最常见的情况是 in-place 模式的 SkillFS 挂载 —— 此时 source 和 mountpoint 是同一个目录。
先卸载再操作：

```bash
skillfs stop /path/to/workspace      # in-place SkillFS 挂载
fusermount3 -u /path/to/workspace    # 其他 FUSE 挂载
```

该约束作用于 `init`，以及在未纳管路径上首次执行的 `checkpoint`（会自动初始化）。工作区
初始化完成之后，后续的 `checkpoint`、`rollback`、`list`、`diff` 都不受影响。

被拒绝的只有工作区根目录本身。工作区**内部**的嵌套挂载不会阻止 `init`，但结果通常不是
你想要的：挂载会留在 `init` 改名移走的备份目录上，新工作区里只有挂载内容的普通副本 ——
后续写入落在副本上而不是挂载的文件系统里，两边会静默分叉。初始化前先卸载嵌套挂载，
或让挂载点保持在工作区目录树之外。

### 回滚到首次对话之前的快照会阻断 OpenClaw

OpenClaw 首次运行时会向工作区种入一组基线文件（AGENTS.md、BOOTSTRAP.md、SOUL.md、
IDENTITY.md、USER.md；2026.7.x 还会种入 HEARTBEAT.md 与 TOOLS.md），
并为该事件保留一条 attestation 记录。如果回滚到 **OpenClaw 首次对话之前**打的快照
（即不含这些基线文件的快照），OpenClaw 会把基线文件的消失误判为工作区被误删，拒绝工作：

```
WorkspaceVanishedError: OpenClaw workspace appears to have disappeared ...
Refusing to reseed BOOTSTRAP.md over a recently attested workspace.
```

**强烈不建议回滚到 OpenClaw 首次对话之前打的快照。** 首次对话完成后立即打一个快照
（`ws-ckpt checkpoint -w ...`），此后所有回滚的目标快照都对应一个真实使用过的工作区。

该保护实际检查的是 survival evidence（存活证据），而非快照的拍摄时间：只要目标快照保留了
OpenClaw 认得的内容，回滚就是安全的。被认可的证据按版本区分：

- 所有版本：
  - 仍然存在的 BOOTSTRAP.md（setup 尚未完成）；
  - 与模板不同的任意 profile 文件（SOUL.md、IDENTITY.md、USER.md）；
  - `memory/` 目录（或 MEMORY.md）；
  - 已安装的 skill（`skills/<name>/SKILL.md`）；
  - 种入的基线文件全部仍在、且与 OpenClaw 生成时逐字节一致——attestation 中记录了它们的
    hash——因此一个从未定制过、但已正常完成 setup 的工作区同样能通过检查。
- 仅 2026.7.x：内容与生成版本不再一致的必需 bootstrap 文件（AGENTS.md、TOOLS.md 或
  HEARTBEAT.md）。
- 仅 2026.8.1 及以后：被定制过的 AGENTS.md（内容既不同于生成版本、也不同于模板）。这些
  版本不再种入 TOOLS.md 与 HEARTBEAT.md，两者在该版本下也永远不算证据。

由此有两个推论：

- 只有当快照丢掉了**全部**上述证据时才会触发该错误——典型情形是回到基线文件种入之前的
  状态，或这些文件后来被删除、且没有留下 memory、skills 或定制过的文件。仅仅从未定制过
  工作区并不会触发：与生成内容一致的基线文件本身就是证据。
- 不要把安全等同于「快照包含完整的一组种子文件」：OpenClaw 在 setup 完成后会删除
  BOOTSTRAP.md，且 2026.8.1 起 `openclaw-workspace-state.json` 仅是 legacy 迁移输入——
  正常的对话后快照两者都不含，但依然是安全的。

**恢复方法（按 OpenClaw 版本选择）：**

- OpenClaw 2026.7.x 及更早版本：删除该工作区对应的 attestation 记录。记录文件名是工作区
  **归一化后**绝对路径的 SHA-256；OpenClaw 会依次在其状态目录（遵循 `OPENCLAW_STATE_DIR`）、
  effective home 目录（遵循 `OPENCLAW_HOME`，默认 `$HOME`）下的 `.openclaw` 与 legacy
  `.clawdbot` 状态目录、以及工作区旁路径查找记录。OpenClaw 用 Node.js 的 `path.resolve`
  归一化路径（折叠 `..` 段与重复斜杠）并展开 env 覆盖值开头的 `~`，因此下面的命令整体在
  `node` 内运行：由它推导出完全相同的路径并亲自删除记录，路径内容不会再被 shell 二次求值
  （npm 方式安装的 OpenClaw CLI 所在环境必有 Node.js）。

  有一个输入是命令无法自行推导的：以 `openclaw --profile <name>` 启动的 agent，其状态目录
  是 `<effective home>/.openclaw-<name>`（`--dev` 等价于 `--profile dev`），而这个选择只
  存在于 agent 进程内部——在 shell 里 export `OPENCLAW_PROFILE` 并**不会**改变状态目录，
  只有命令行 flag 才会。与其猜错目录、静默清理无效位置，命令会在 `OPENCLAW_STATE_DIR`
  未设置、且 effective home 下存在多个 `.openclaw*` 状态目录时直接报错退出。此时先 export
  agent 实际使用的状态目录，再重新执行：

  ```bash
  export OPENCLAW_STATE_DIR="$HOME/.openclaw-team"   # agent 以 `openclaw --profile team ...` 运行
  ```

  恢复命令：

  ```bash
  WS='/path/to/workspace'   # 工作区绝对路径（单引号赋值：路径中的 $、反引号等保持字面值）
  node -e '
    const crypto = require("crypto"), fs = require("fs"), os = require("os"), path = require("path");
    const env = process.env;
    if (!process.argv[1]) { console.error("Set WS to the workspace path and pass it to this command."); process.exit(1); }
    const rawHome = (env.OPENCLAW_HOME || "").trim();
    const OC_HOME = rawHome
      ? path.resolve(rawHome.replace(/^~(?=$|[\\/])/, os.homedir()))
      : os.homedir();
    const WS = path.resolve(process.argv[1]);
    const HASH = crypto.createHash("sha256").update(WS).digest("hex");
    const sdOverride = (env.OPENCLAW_STATE_DIR || "").trim();
    let SD;
    if (sdOverride) {
      SD = path.resolve(sdOverride.replace(/^~(?=$|[\\/])/, OC_HOME));
    } else {
      let candidates = [];
      try {
        candidates = fs.readdirSync(OC_HOME, { withFileTypes: true })
          .filter((e) => e.isDirectory() && (e.name === ".openclaw" || e.name.startsWith(".openclaw-")))
          .map((e) => path.join(OC_HOME, e.name));
      } catch {}
      if (candidates.length > 1) {
        console.error("Multiple OpenClaw state directories exist under " + OC_HOME + ":\n"
          + candidates.map((d) => "  " + d).join("\n") + "\n"
          + "An agent started with `openclaw --profile <name>` (or `--dev`) keeps its state in\n"
          + "<effective home>/.openclaw-<name>, and that value exists only inside the agent process.\n"
          + "Export the state directory the agent actually used, then rerun this command:\n"
          + "  export OPENCLAW_STATE_DIR=" + OC_HOME + "/.openclaw-<name>");
        process.exit(1);
      }
      SD = candidates[0] || path.join(OC_HOME, ".openclaw");
    }
    const stateDirs = [...new Set([SD, path.join(OC_HOME, ".openclaw"), path.join(OC_HOME, ".clawdbot")])];
    const targets = stateDirs.map((d) => path.join(d, "workspace-attestations", HASH + ".attested"));
    targets.push(WS + ".attested");
    console.log("workspace:   " + WS);
    console.log("state dir:   " + SD);
    let failed = false;
    for (const t of targets) {
      try {
        if (fs.existsSync(t)) { fs.rmSync(t); console.log("removed:     " + t); }
        else { console.log("not present: " + t); }
      } catch (err) {
        failed = true;
        console.error("FAILED:      " + t + " (" + err.message + ")");
      }
    }
    if (failed) process.exit(1);
  ' "$WS"
  ```

  删除后重新运行 agent 会话，OpenClaw 会重新种入基线文件并开始新的 attestation。

- OpenClaw 2026.8.1 及以上：attestation 记录已迁入 OpenClaw 的状态 SQLite 数据库，
  目前没有只删除这一条记录的命令。报错信息建议执行完整的 OpenClaw 重置
  （`openclaw reset --scope full`），但那会删除**所有** agent 的 workspace 以及整个
  OpenClaw 状态目录——包括凭据、会话和已安装的 plugin——破坏面远超本场景所需。要
  恢复工作区的正常行为，建议再次回滚到工作区真实使用过期间打的快照（即满足上述存活
  证据条件的任意快照）：

  ```bash
  ws-ckpt rollback -w /path/to/workspace -s <snapshot-id>
  ```

  回滚后工作区立即可用。另外，guard 状态本身会过期：最后一次状态写入 24 小时后，OpenClaw
  会清掉过期的 attestation 并在下一次 agent 运行时重新种入工作区，因此保留 pre-baseline
  快照、24 小时后重试同样可行，无需完整重置。若不想等 24 小时、需要立即解除阻断，目前
  只能执行上文所述的完整 OpenClaw 重置。注意：无论是等待过期还是执行重置，下一次 agent
  运行时基线文件都会被重新种入工作区。

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
