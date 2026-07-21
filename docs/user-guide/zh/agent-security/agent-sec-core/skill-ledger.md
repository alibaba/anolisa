# Skill Ledger 用户使用手册

Skill Ledger 是 agent-sec-core 的安全子系统，为 AI Agent Skill 提供文件哈希、扫描结果和密码学签名的版本链，帮助发现 Skill 被篡改或注入恶意内容。默认快速扫描由内置静态扫描器自动执行；可选深度扫描由 Agent 按 `skill-vetter` 协议驱动执行。

---

## 第一部分：快速体验

### 核心概念

| 概念 | 说明 |
|------|------|
| **Manifest** | JSON 记录（`.skill-meta/latest.json`），包含文件哈希、扫描结果和数字签名；由 `scan`、`certify` 或 `init` baseline 创建和更新 |
| **版本链** | 只追加的账本——每个版本通过 `previousManifestSignature` 链接上一版本，形成防篡改历史 |
| **状态** | 每个 Skill 的安全状态：`pass` ✅ · `none` 🆕 · `drifted` 🔄 · `warn` ⚠️ · `deny` 🚨 · `tampered` 🔴 |

### 1. 初始化签名密钥

```bash
# 初始化密钥，并为已覆盖目录中的 Skill 建立快速扫描 baseline
agent-sec-cli skill-ledger init
```

密钥存放位置：

| 文件 | 路径 | 权限 |
|------|------|------|
| 私钥文件 | `~/.local/share/agent-sec/skill-ledger/key.enc` | 0600；默认未加密，`--passphrase` 时加密 |
| 公钥 | `~/.local/share/agent-sec/skill-ledger/key.pub` | 0644 |

如需口令保护私钥：

```bash
# 交互式输入口令
agent-sec-cli skill-ledger init --passphrase

# 或通过环境变量（适用于 CI）
SKILL_LEDGER_PASSPHRASE="your-secret" agent-sec-cli skill-ledger init --passphrase
```

### 2. 检查 Skill 完整性

```bash
agent-sec-cli skill-ledger check /path/to/your-skill
```

输出 JSON，关键字段为 `status`：

| 状态 | 含义 |
|------|------|
| `none` 🆕 | 从未扫描——没有可验证的签名 manifest |
| `pass` ✅ | 文件未变 + 签名有效 + 扫描通过 |
| `drifted` 🔄 | Skill 文件已变更（fileHashes 不匹配） |
| `warn` ⚠️ | 签名有效，但上次扫描存在低风险发现 |
| `deny` 🚨 | 签名有效，但上次扫描存在高危发现 |
| `tampered` 🔴 | manifest 签名校验失败——元数据可能被伪造 |

### 3. 快速扫描 + 签名认证

默认认证路径使用内置快速扫描器，不依赖 LLM。对单个 Skill 执行：

```bash
agent-sec-cli skill-ledger scan /path/to/your-skill
```

扫描完成后，可重新检查状态：

```bash
agent-sec-cli skill-ledger check /path/to/your-skill
```

如需更完整的语义审查，可通过 Agent 触发深度扫描。Agent 读取内置的 `skill-vetter-protocol.md` 扫描协议，逐文件对目标 Skill 进行四阶段审查（来源验证 → 代码审查 → 权限边界评估 → 风险分级），将结果写入 findings JSON 文件。随后将 findings 文件传入 `certify` 完成签名认证：

```bash
agent-sec-cli skill-ledger certify /path/to/your-skill \
  --findings /tmp/skill-vetter-findings-your-skill.json \
  --scanner skill-vetter \
  --delete-findings
```

`scan` 会运行内置快速扫描器并签名入账；`certify` 则只导入外部 findings。`certify` 会依次：

1. 验证文件一致性（文件变更时自动创建新版本）
2. 规范化 findings 并合并到 manifest 的 `scans[]` 数组
3. 聚合 `scanStatus`（`pass` / `warn` / `deny`）
4. 重新签名并写入 `.skill-meta/latest.json`

输出示例：

```json
{
  "versionId": "v000002",
  "scanStatus": "pass",
  "newVersion": true,
  "skillName": "your-skill"
}
```

### 4. 查看整体安全状况

```bash
# 查看 skill-ledger 系统整体状况（密钥、配置、所有 Skill 健康度）
agent-sec-cli skill-ledger status

# 包含每个 Skill 的详细状态
agent-sec-cli skill-ledger status --verbose
```

`status` 输出 JSON，包含三个区块：

| 区块 | 说明 |
|------|------|
| `keys` | 签名密钥状态（是否初始化、指纹、是否加密、归档密钥数） |
| `config` | 配置摘要（默认目录、managedSkillDirs 模式数、已注册扫描器） |
| `skills` | 聚合健康度（已发现 Skill 数、各状态计数、整体 health 标签） |

`health` 标签含义：`healthy`（没有 critical/attention 状态，且不是全部 none；可能包含 pass/none 混合）、`unscanned`（全部 none）、`attention`（存在 drifted/warn）、`critical`（存在 deny/tampered/error）、`empty`（无已注册 Skill）。

使用 `--verbose` 时会额外输出 `results` 数组，包含每个 Skill 的详细检查结果。

### 5. 审计完整版本链

深度验证全部历史版本——校验哈希完整性、签名有效性和版本链链接：

```bash
agent-sec-cli skill-ledger audit /path/to/your-skill

# 同时验证快照文件哈希
agent-sec-cli skill-ledger audit /path/to/your-skill --verify-snapshots
```

### 6. Agent 驱动扫描（推荐方式）

最自然的使用方式是通过 AI Agent 自然语言触发。默认“扫描”会执行快速扫描；只有用户明确要求深度扫描，或在快速扫描后确认继续，才执行 `skill-vetter` 深度扫描：

| 说法 | 效果 |
|------|------|
| "扫描 /path/to/skill" | 对指定 Skill 执行快速扫描认证 |
| "扫描所有 skill" | 批量快速扫描 `config.json` 中配置的所有 Skill |
| "深度扫描 /path/to/skill" | 按 `skill-vetter` 协议执行逐文件深度审查并认证 |
| "检查 skill 状态" | 仅输出状态分诊表，不执行扫描 |

Skill 工作流：

- **Phase 1**（环境准备与状态查看）：校验 CLI、密钥，解析目标 Skill，输出分诊表
- **Phase 2**（快速扫描认证）：调用内置 `code-scanner` 与 `static-scanner`，再签名写入 manifest
- **Phase 3**（可选深度扫描）：`skill-vetter` 四阶段审查——来源验证 → 代码审查 → 权限边界评估 → 风险分级，再通过 `certify --findings` 写入版本链

---

## 第二部分：通过 SkillFS 激活、用户决策与宿主 Hook Policy 保护 Skill 安全

### 架构概览

Skill Ledger 推荐与 SkillFS 联合使用：SkillFS 捕获 Skill 变更，通知 Skill Ledger daemon 扫描并刷新 `.skill-meta/activation.json`/xattr。宿主 hook/capability 默认仍可挂载，默认 `policy = "ask"`；当统一 exposure summary 有 `message` 时提示用户，没有 `message` 或用户已经做过决策时静默。

```
┌──────────────────────────────────────────────────┐
│                  Agent 运行时                      │
│                                                   │
│  ┌──────────────┐      ┌──────────────────────┐   │
│  │  SkillFS     │      │  skill-ledger        │   │
│  │  变更捕获      │      │  SKILL.md            │   │
│  │               │      │  (按需深度扫描)       │   │
│  │      │        │      └──────────┬───────────┘   │
│  │      ▼        │                 │               │
│  │ daemon notify │                 │               │
│  │      │        │                 │               │
│  │      ▼        │                 │               │
│  │ activation    │                 │               │
│  │ refresh       │                 │               │
│  └──────┤────────┘                 │               │
│         ▼                         ▼               │
│  ┌──────────────────────────────────────────┐     │
│  │       agent-sec-cli skill-ledger          │     │
│  │   show / export / decide / scan / certify │     │
│  └──────────────────────────────────────────┘     │
│                      │                            │
│                      ▼                            │
│           .skill-meta/latest.json                 │
│           .skill-meta/activation.json + xattr     │
└───────────────────────────────────────────────────┘
```

- **推荐路径——SkillFS + daemon activation**：SkillFS 负责发现 Skill 文件变化；daemon 根据最新签名 manifest、用户决策和 activation policy 刷新可执行 activation 目标。Agent 运行时读取 activation metadata，而不是默认依赖宿主 hook 前置检查。
- **兼容路径——宿主 hook/capability policy**：OpenClaw、Hermes 和 copilot-shell 可在 Skill 加载前调用 `agent-sec-cli skill-ledger show`；Qoder CLI 则在 `Skill` tool 执行前对解析出的本地绝对目录调用只读的 `agent-sec-cli skill-ledger check`。默认 `ask` 请求用户确认；也可显式配置 `warn` / `debug` / `block`。
- **Agent 驱动扫描**：`scan` 执行内置快速扫描并签名；`skill-ledger` Skill 在用户要求深度扫描时驱动完整的四阶段安全审查，并通过 `certify --findings` 导入结果。**按需触发**，由用户请求发起。

### 推荐路径：SkillFS + daemon activation

**工作原理：**

启用 SkillFS 后，Skill Ledger 的运行态入口由 daemon 处理：

1. SkillFS 捕获 Skill 目录创建、更新、删除或内容变更。
2. SkillFS 通知 Skill Ledger daemon 的 `skill_ledger.skillfs_notify_change` 接口。
3. daemon 根据签名 manifest、当前文件状态、用户决策和 activation policy 刷新 `.skill-meta/activation.json`，并尽力同步写入 xattr。
4. 若当前风险版本不可直接激活，activation metadata 会指向上一个可信 `pass` / `warn` snapshot；若没有可信 fallback，则指向安全 pending review stub；用户 `block` 决策或 fail-safe 场景才写 `target: null`。

### 兼容路径：Hook / capability policy

当 Agent 加载 Skill 时，OpenClaw、Hermes 和 copilot-shell hook 会解析 Skill 目录，执行 `agent-sec-cli skill-ledger show <skill_dir>`，并由统一 `policy` 控制可见行为。这些 hook 只消费 summary 中的 `message`：

| Policy | 行为 |
|--------|------|
| `ask` | 默认值。`message == null` 静默放行；`message != null` 时请求用户确认或使用宿主 approval UI。 |
| `warn` | `message == null` 静默放行；`message != null` 时展示 warning 并放行。 |
| `debug` | `message != null` 时只写 debug 诊断并放行。 |
| `block` | `message != null` 时直接阻断，并把 message 作为原因或告警信息。 |

`message` 的触发规则由 Skill Ledger 统一决定：用户已有 `allow` / `always_allow` / `rollback` / `block` 决策时不提示；latest 为 `pass` 或 `warn` 且可直接暴露时不提示；无用户决策且 latest 为 `deny` / `none` / `drifted` / `tampered` 时提示，并说明当前 active 是 fallback 版本还是安全 pending review stub。`latestStatus=unmanaged` 表示当前 daemon 无法管理该 root，无法写 `.skill-meta` 或记录用户决策，因此只作为诊断返回，`message=null`，所有 hook policy 包括 `block` 都静默放行。

Qoder CLI 是低层完整性门禁：plugin 为 `Skill` tool 注册独立的 `PreToolUse` hook，根据事件中的绝对 `cwd` 建立 user → project 目录表，解析 `SKILL.md` frontmatter `name`（无 frontmatter 时回退目录名），完成 canonical path 和根目录边界校验后执行 `skill-ledger check <skill_dir>`。frontmatter 存在但 `name` 缺失、歧义或使用 hook 无法安全解析的 YAML scalar 时，不会降级为非本地 Skill，而是按当前 policy 处理。`pass` 静默放行；`none` / `drifted` / `warn` / `deny` / `tampered` 以及 `error` 按 `ask` / `debug` / `warn` / `block` policy 请求确认、仅记 debug、提示后放行或阻断。CLI 不可用、执行失败、超时或输出不可解析也按该四档 policy 处理，而不是固定 fail-open。

OpenClaw 默认 `enabled=true, policy="ask"`；Hermes 默认 `enabled=true, policy="ask"`；copilot-shell 和 Qoder CLI 默认 manifest 注册 Skill Ledger PreToolUse hook，并通过 `SKILL_LEDGER_HOOK_POLICY` 控制 policy。除 Qoder CLI 上述低层门禁外，其它兼容 hook 在 CLI 基础设施异常时保持 fail-open，避免阻断 Skill 加载。

copilot-shell hook 当前仅覆盖 project / user / system 三类目录：`<cwd>/.copilot-shell/skills/`、`~/.copilot-shell/skills/`、`/usr/share/anolisa/skills/`。若 Skill 来自 custom、extension、remote 或其它路径，hook 会 fail-open 并跳过 skill-ledger 检查；OpenClaw 插件则按读取到的 `SKILL.md` 路径提取 Skill 目录。

批量认证或安装后认证场景中，建议先完成目录定位和认证，再让 Agent 读取未认证 Skill 内容：批量认证前避免主动读取未认证 Skill 的 `SKILL.md` 或辅助文件；安装成功后应先定位最终本地目录，确认包含 `SKILL.md`，再执行快速扫描认证。

**OpenClaw 启用方式**：

```json
{
  "capabilities": {
    "skill-ledger": {
      "enabled": true,
      "policy": "ask"
    }
  }
}
```

**Hermes 启用方式**：

```toml
[capabilities.skill-ledger]
enabled = true
timeout = 5
policy = "ask"
enable_block = false
```

**copilot-shell 配置方式**：默认 Cosh manifest 已注册 `skill-ledger` hook。默认 policy 为 `ask`；如需 warning-only、debug-only 或强拒绝，可设置 `SKILL_LEDGER_HOOK_POLICY=warn` / `debug` / `block`。该环境变量应由可信宿主或部署环境设置，不应由 Skill、项目脚本或不可信 shell 启动逻辑设置；如需防止本地 shell profile 被篡改后降级策略，后续应迁移到可信宿主配置源。

**Qoder CLI 配置方式**：安装 `qoder-plugin` 后，plugin 自动注册 matcher 为 `Skill` 的 `PreToolUse` hook。默认 policy 为 `ask`；可由可信启动环境设置 `SKILL_LEDGER_HOOK_POLICY=debug` / `warn` / `block`，并通过 `SKILL_LEDGER_TIMEOUT` 调整 CLI 超时（默认 5 秒）。hook 覆盖 `~/.qoder/skills/` 和 `<cwd>/.qoder/skills/` 下的本地 Skill，用户级同名 Skill 优先；仅在两个目录表都可信解析且没有匹配时，才把调用视为内置、plugin 或 remote Skill，放行并记录 debug。hook 不自动执行 `init` 或 `scan`，未签名 Skill 会以 `none` 状态进入 policy；完成审查后需显式执行 `agent-sec-cli skill-ledger scan <skill_dir>`。

Skill Ledger 全局 `activationPolicy` 属于 SkillFS/daemon activation；这里的 hook `policy` 只控制宿主 hook/capability 的用户可见行为和日志等级。

### 风险 Skill 用户审查与决策

当 hook 或 `show` 提示当前 skill 需要用户审查时，先查看统一暴露摘要：

```bash
agent-sec-cli skill-ledger show /path/to/skill
```

重点字段：

| 字段 | 含义 |
|------|------|
| `latestStatus` | 最新 skill 根目录或最新签名版本的状态 |
| `activeVersionId` | 当前真实暴露给 SkillFS 的版本；为 `null` 时表示没有真实 active version |
| `target` | SkillFS 当前读取的 target；pending 状态会指向 `.skill-meta/versions/__pending_decision__.snapshot` |
| `userDecision` | 当前命中的用户决策；为 `null` 表示尚未决策 |
| `message` | 需要提示用户的信息；为 `null` 时 hook 静默 |

若需要完整查看不可见的风险版本，导出 latest snapshot、manifest 和 findings：

```bash
agent-sec-cli skill-ledger export /path/to/skill --version latest --output /tmp/skill-review
```

审查后通过统一 `decide` 命令选择：

```bash
# 允许当前具体版本；不继承到未来版本
agent-sec-cli skill-ledger decide /path/to/skill --action allow --reason "reviewed manually"

# 允许当前及未来版本，直到用户主动改成其它决策或清除
agent-sec-cli skill-ledger decide /path/to/skill --action always_allow --reason "trusted source"

# 完全隐藏当前 skill；该 block 不继承到未来新版本
agent-sec-cli skill-ledger decide /path/to/skill --action block --reason "unsafe behavior"

# 回退到指定版本；不写 --version 时默认选择当前真实 active version
agent-sec-cli skill-ledger decide /path/to/skill --action rollback --version v000001 --reason "use previous trusted version"

# 清除 latest manifest 上的用户决策，恢复全局 activation 行为
agent-sec-cli skill-ledger decide /path/to/skill --clear
```

注意：hook 的 `ask` 确认只允许本次宿主操作继续，不等价于 Skill Ledger 的 `allow`。只有 `decide` 会改变后续 activation target。

### Agent 驱动深度扫描

#### 配置 Skill 目录（批量扫描使用）

默认已包含五个内置目录：`~/.openclaw/skills/*`、`~/.copilot-shell/skills/*`、`~/.hermes/skills/**`、`~/.qoder/skills/*`、`/usr/share/anolisa/skills/*`。项目级 Qoder 目录不作为相对默认项；对项目 Skill 显式执行 `scan` 或 `certify` 后，其绝对目录会沿用自动记忆机制写入 `managedSkillDirs`。如需添加其它目录，创建或编辑 `~/.config/agent-sec/skill-ledger/config.json`：

```json
{
  "enableDefaultSkillDirs": true,
  "managedSkillDirs": [
    "/opt/custom-skills/*",
    "/opt/custom-skills/my-skill"
  ]
}
```

默认目录默认启用；`managedSkillDirs` 用于 skill-ledger 动态管理或用户额外配置的目录，会追加到默认目录之后（自动去重）。如需隔离运行，可将 `enableDefaultSkillDirs` 设为 `false`。

- `"path/*"` — glob 模式：每个包含 `SKILL.md` 的子目录视为一个 Skill
- `"path/to/skill"` — 单个 Skill 目录（同样需包含 `SKILL.md`）

不存在的目录会被静默忽略。此外，对 Skill 执行 `scan` 或 `certify` 时，未收录的目录会自动追加到配置中，方便后续 `--all` 批量操作。`check` 是只读状态检查，不会写入配置。

#### 定时执行默认快速扫描

如果希望定期刷新默认快速扫描结果，可以把 `scan --all` 放入 cron。`scan --all` 会自动跳过文件未变且已有完整扫描结果的 Skill，只补扫新增、变更、缺少扫描结果或 manifest 异常的 Skill。

无口令密钥场景：

```bash
mkdir -p "$HOME/.local/state/agent-sec"
AGENT_SEC_CLI="$(command -v agent-sec-cli)"
CRON_LINE="0 3 * * * $AGENT_SEC_CLI skill-ledger scan --all >> $HOME/.local/state/agent-sec/skill-ledger-scan.log 2>&1"
(crontab -l 2>/dev/null | grep -Fv "skill-ledger scan --all"; echo "$CRON_LINE") | crontab -
```

使用口令保护私钥时，定时任务需要提供 `SKILL_LEDGER_PASSPHRASE`。下面的命令会把口令以明文写入当前用户的 crontab 和系统 cron spool，请只在可信单用户环境中使用；更安全的做法是使用默认无口令密钥，或通过本机 secret manager / 受限权限文件包装 `scan --all`。

```bash
read -rsp "SKILL_LEDGER_PASSPHRASE: " SKILL_LEDGER_PASSPHRASE; echo
mkdir -p "$HOME/.local/state/agent-sec"
AGENT_SEC_CLI="$(command -v agent-sec-cli)"
CRON_LINE="0 3 * * * SKILL_LEDGER_PASSPHRASE='$SKILL_LEDGER_PASSPHRASE' $AGENT_SEC_CLI skill-ledger scan --all >> $HOME/.local/state/agent-sec/skill-ledger-scan.log 2>&1"
(crontab -l 2>/dev/null | grep -Fv "skill-ledger scan --all"; echo "$CRON_LINE") | crontab -
unset SKILL_LEDGER_PASSPHRASE
```

查看已安装的定时任务：

```bash
crontab -l
```

#### 触发扫描

通过自然语言向 Agent 发出指令即可。默认扫描执行 Phase 1 → Phase 2；用户明确要求深度扫描时执行 Phase 1 → Phase 3。

**深度扫描规则表（skill-vetter）：**

| 级别 | 规则 ID | 检测目标 |
|------|---------|---------|
| deny | `dangerous-exec` | 危险进程执行（`child_process`、`subprocess`） |
| deny | `dynamic-code-eval` | 动态代码执行（`eval()`、`new Function()`） |
| deny | `env-harvesting` | 环境变量批量采集 + 网络发送 |
| deny | `crypto-mining` | 挖矿特征（`stratum`、`xmrig` 等） |
| deny | `credential-access` | 凭据与敏感文件访问（`~/.ssh/`、`.env`） |
| deny | `system-modification` | 系统文件篡改（`/etc/`、crontab） |
| deny | `prompt-override` | Prompt 覆盖指令 |
| deny | `hidden-instruction` | 隐藏指令（零宽字符、HTML 注释） |
| warn | `obfuscated-code` | 代码混淆（超长行、base64 + decode） |
| warn | `suspicious-network` | 可疑网络连接（直连 IP、非标准端口） |
| warn | `exfiltration-pattern` | 数据外泄模式（文件读取 + 网络发送组合） |
| warn | `agent-data-access` | Agent 身份数据访问（`MEMORY.md` 等） |
| warn | `unauthorized-install` | 未声明的包安装 |
| warn | `unrestricted-tool-use` | 无约束工具使用指令 |
| warn | `external-fetch-exec` | 外部获取执行（`curl | bash`） |
| warn | `privilege-escalation` | 权限提升（`sudo`、`chmod 777`） |

### 实战场景

#### 场景 A：加载第三方 Skill 时检测篡改

```
# SkillFS/daemon 或宿主 hook 检测到异常状态
[skill-ledger] 🚨 Skill 'third-party-tool' metadata signature verification failed
```

告警表明有人可能修改了 manifest，将 `scanStatus` 从 `deny` 改为 `pass` 以绕过安全检查。

#### 场景 B：Skill 更新后检测漂移

```bash
agent-sec-cli skill-ledger check /path/to/my-skill
# → {"status": "drifted", "added": [...], "modified": [...]}
```

更新 Skill 后状态变为 `drifted`。触发重新扫描恢复到 `pass`：

```
扫描 /path/to/my-skill
```

#### 场景 C：审计历史完整性

```bash
agent-sec-cli skill-ledger audit /path/to/my-skill --verify-snapshots
```

逐版本验证：哈希完整性 → 签名有效性 → 版本链链接 → 快照一致性。

---

## 命令速查表

| 命令 | 用途 |
|------|------|
| `agent-sec-cli skill-ledger init` | 初始化密钥，并为已覆盖 Skill 建立快速扫描 baseline |
| `agent-sec-cli skill-ledger init --no-baseline` | 只初始化密钥，不扫描 Skill |
| `agent-sec-cli skill-ledger check <dir>` | 检查完整性状态（JSON 输出） |
| `agent-sec-cli skill-ledger show <dir>` | 展示 latest、active、用户决策、activation target、findings 与告警信息 |
| `agent-sec-cli skill-ledger export <dir> --version latest --output <path>` | 导出指定 snapshot、manifest 和 findings 供完整审查 |
| `agent-sec-cli skill-ledger decide <dir> --action allow|always_allow|block|rollback` | 写入用户决策并刷新 activation |
| `agent-sec-cli skill-ledger decide <dir> --clear` | 清除 latest manifest 上的用户决策 |
| `agent-sec-cli skill-ledger scan <dir>` | 执行快速扫描并签名写入 manifest |
| `agent-sec-cli skill-ledger scan --all` | 对所有已发现 Skill 执行补齐式快速扫描 |
| `agent-sec-cli skill-ledger certify <dir> --findings <file>` | 将深度扫描 findings 签名写入 manifest |
| `agent-sec-cli skill-ledger status` | 查看整体安全状况（密钥、配置、Skill 健康度） |
| `agent-sec-cli skill-ledger status --verbose` | 查看整体安全状况（含每个 Skill 详细结果） |
| `agent-sec-cli skill-ledger audit <dir>` | 深度验证版本链 |
| `agent-sec-cli skill-ledger list-scanners` | 查看已注册的扫描器列表 |

## 关键路径

| 路径 | 用途 |
|------|------|
| `~/.local/share/agent-sec/skill-ledger/key.enc` | 私钥文件（默认未加密，`--passphrase` 时加密） |
| `~/.local/share/agent-sec/skill-ledger/key.pub` | 公钥 |
| `~/.local/share/agent-sec/skill-ledger/keyring/` | 归档的历史公钥（密钥轮换后） |
| `~/.config/agent-sec/skill-ledger/config.json` | 配置文件（managedSkillDirs、scanners） |
| `<skill_dir>/.skill-meta/latest.json` | 当前 manifest（由 `scan`、`certify` 或 `init` baseline 写入） |
| `<skill_dir>/.skill-meta/versions/` | 版本链历史 |
