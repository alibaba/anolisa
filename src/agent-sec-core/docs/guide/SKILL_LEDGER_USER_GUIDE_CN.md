# Skill Ledger 用户使用手册

Skill Ledger 是 agent-sec-core 的安全子系统，为 AI Agent Skill 提供文件哈希、扫描结果和密码学签名的版本链，帮助发现 Skill 被篡改或注入恶意内容。默认快速扫描由内置静态扫描器自动执行；可选深度扫描由 Agent 按 `skill-vetter` 协议驱动执行。

---

## 第一部分：快速体验

### 核心概念

| 概念 | 说明 |
|------|------|
| **Manifest** | JSON 记录（`.skill-meta/latest.json`），包含文件哈希、扫描结果和数字签名；首次 `check` 创建的 baseline 可能尚未签名，`scan` 或 `certify` 后会补签 |
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
| `none` 🆕 | 从未扫描——首次检查时自动创建未签名基线 manifest |
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

## 第二部分：通过 Skill 调用与 Hook 保护 Skill 安全

### 架构概览

Skill Ledger 提供**两层防护**协同工作：

```
┌──────────────────────────────────────────────────┐
│                  Agent 运行时                      │
│                                                   │
│  ┌──────────────┐      ┌──────────────────────┐   │
│  │  Hook 层      │      │  skill-ledger        │   │
│  │  (自动守卫)    │      │  SKILL.md            │   │
│  │               │      │  (按需深度扫描)       │   │
│  │ ┌──────────┐  │      └──────────┬───────────┘   │
│  │ │ OpenClaw  │  │               │               │
│  │ │ Plugin    │  │               │               │
│  │ ├──────────┤  │               │               │
│  │ │ cosh Hook │  │               │               │
│  │ │ (Python)  │  │               │               │
│  │ └────┬─────┘  │               │               │
│  └──────┤────────┘               │               │
│         ▼                         ▼               │
│  ┌──────────────────────────────────────────┐     │
│  │       agent-sec-cli skill-ledger          │     │
│  │   check / scan / certify / audit / status │     │
│  └──────────────────────────────────────────┘     │
│                      │                            │
│                      ▼                            │
│           .skill-meta/latest.json                 │
│           (SignedManifest + 版本链)                 │
└───────────────────────────────────────────────────┘
```

- **第一层——自动 Hook（实时守卫）**：
  - **OpenClaw**：插件拦截所有对 `SKILL.md` 的 `read` 调用，在 Skill 加载前自动运行 `check`。
  - **copilot-shell**：Python hook 脚本（`cosh-extension/hooks/skill_ledger_hook.py`）通过 `PreToolUse` 事件在 Skill 调用前自动运行 `check`。
  - 两者采用相同默认策略：`pass` 静默放行，`warn`/`error`/`unknown` 告警放行，`none`/`drifted`/`deny`/`tampered` 要求用户确认。插件或扩展加载且能力未禁用时生效。
- **第二层——Agent 驱动扫描**：`scan` 执行内置快速扫描并签名；`skill-ledger` Skill 在用户要求深度扫描时驱动完整的四阶段安全审查，并通过 `certify --findings` 导入结果。**按需触发**，由用户请求发起。

### 第一层：自动 Hook 防护

**工作原理：**

OpenClaw 安全插件注册了一个 `before_tool_call` hook（优先级 80）。当 Agent 调用 `read` 读取任何 `SKILL.md` 文件时：

1. Hook 从文件路径提取 Skill 目录
2. 确保签名密钥存在（缺失时自动初始化）
3. 执行 `agent-sec-cli skill-ledger check <skill_dir>`
4. 根据状态执行默认策略：

| 状态 | 默认行为 | 输出 |
|------|---------|------|
| `pass` | 静默放行 | 无 |
| `warn` | 放行 + 告警 | `⚠️ Skill 'skill-name' has low-risk findings — review recommended` |
| `error` | 放行 + 告警 | `⚠️ Skill 'skill-name' check returned an error — review recommended` |
| `unknown` | 放行 + 告警 | `⚠️ Skill 'skill-name' returned an unknown status — review recommended` |
| `none` | 用户确认 | `⚠️ Skill 'skill-name' has not been security-scanned yet` |
| `drifted` | 用户确认 | `⚠️ Skill 'skill-name' content has changed since last scan` |
| `deny` | 用户确认 | `🚨 Skill 'skill-name' has high-risk findings — immediate review recommended` |
| `tampered` | 用户确认 | `🚨 Skill 'skill-name' metadata signature verification failed` |

OpenClaw 在需要确认时返回 `requireApproval`；copilot-shell 在需要确认时返回 `decision: "ask"`。CLI 不可用、执行失败、超时或输出不可解析时保持 fail-open，避免基础设施异常阻断 Skill 加载。

copilot-shell hook 当前仅覆盖 project / user / system 三类目录：`<cwd>/.copilot-shell/skills/`、`~/.copilot-shell/skills/`、`/usr/share/anolisa/skills/`。若 Skill 来自 custom、extension、remote 或其它路径，hook 会 fail-open 并跳过 skill-ledger 检查；OpenClaw 插件则按读取到的 `SKILL.md` 路径提取 Skill 目录。

批量认证或安装后认证场景中，建议先完成目录定位和认证，再让 Agent 读取未认证 Skill 内容：批量认证前避免主动读取未认证 Skill 的 `SKILL.md` 或辅助文件；安装成功后应先定位最终本地目录，确认包含 `SKILL.md`，再执行快速扫描认证。

**启用方式**：确保 `agent-sec` 插件已加载，且 `skill-ledger` 能力未被显式禁用。插件配置中可通过以下方式禁用：

```json
{
  "capabilities": {
    "skill-ledger": { "enabled": false }
  }
}
```

### 第二层：Agent 驱动深度扫描

#### 配置 Skill 目录（批量扫描使用）

默认已包含三个内置目录：`~/.openclaw/skills/*`、`~/.copilot-shell/skills/*`、`/usr/share/anolisa/skills/*`。如需添加额外目录，创建或编辑 `~/.config/agent-sec/skill-ledger/config.json`：

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

不存在的目录会被静默忽略。此外，对 Skill 执行 `check`、`scan` 或 `certify` 时，未收录的目录会自动追加到配置中，方便后续 `--all` 批量操作。

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
# Agent 加载 Skill → hook 自动触发
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
| `<skill_dir>/.skill-meta/latest.json` | 当前 manifest（首次 `check` baseline 可未签名，`certify` 后签名） |
| `<skill_dir>/.skill-meta/versions/` | 版本链历史 |
