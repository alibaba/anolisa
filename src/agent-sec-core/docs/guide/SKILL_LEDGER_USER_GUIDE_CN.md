# Skill Ledger 用户使用手册

Skill Ledger 是 agent-sec-core 的安全子系统，为 AI Agent Skill 提供密码学签名的版本链，防止 Skill 被篡改或注入恶意内容。安全扫描能力由外部扫描器提供（当前内置 `skill-vetter` 协议定义，扫描由 Agent 驱动执行）。

---

## 第一部分：快速体验

### 核心概念

| 概念 | 说明 |
|------|------|
| **Manifest** | 签名的 JSON 记录（`.skill-meta/latest.json`），包含文件哈希、扫描结果和数字签名 |
| **版本链** | 只追加的账本——每个版本通过 `previousManifestSignature` 链接上一版本，形成防篡改历史 |
| **状态** | 每个 Skill 的安全状态：`pass` ✅ · `none` 🆕 · `drifted` 🔄 · `warn` ⚠️ · `deny` 🚨 · `tampered` 🔴 |

### 1. 初始化签名密钥

```bash
# 生成 Ed25519 签名密钥对（默认无口令，零交互）
agent-sec-cli skill-ledger init-keys
```

密钥存放位置：

| 文件 | 路径 | 权限 |
|------|------|------|
| 加密私钥 | `~/.local/share/skill-ledger/key.enc` | 0600 |
| 公钥 | `~/.local/share/skill-ledger/key.pub` | 0644 |

如需口令保护私钥：

```bash
# 交互式输入口令
agent-sec-cli skill-ledger init-keys --passphrase

# 或通过环境变量（适用于 CI）
SKILL_LEDGER_PASSPHRASE="your-secret" agent-sec-cli skill-ledger init-keys
```

### 2. 检查 Skill 完整性

```bash
agent-sec-cli skill-ledger check /path/to/your-skill
```

输出 JSON，关键字段为 `status`：

| 状态 | 含义 |
|------|------|
| `none` 🆕 | 从未扫描——首次检查时自动创建基线 manifest |
| `pass` ✅ | 文件未变 + 签名有效 + 扫描通过 |
| `drifted` 🔄 | Skill 文件已变更（fileHashes 不匹配） |
| `warn` ⚠️ | 签名有效，但上次扫描存在低风险发现 |
| `deny` 🚨 | 签名有效，但上次扫描存在高危发现 |
| `tampered` 🔴 | manifest 签名校验失败——元数据可能被伪造 |

### 3. 安全扫描 + 签名认证

安全扫描由 AI Agent 加载 `skill-ledger` Skill 后驱动执行——Agent 读取内置的 `skill-vetter-protocol.md` 扫描协议，逐文件对目标 Skill 进行四阶段审查（来源验证 → 代码审查 → 权限边界评估 → 风险分级），将结果写入 findings JSON 文件。详见[第二部分：Agent 驱动深度扫描](#第二层agent-驱动深度扫描)。

扫描完成后，将 findings 文件传入 `certify` 完成签名认证：

```bash
agent-sec-cli skill-ledger certify /path/to/your-skill \
  --findings /tmp/skill-vetter-findings-your-skill.json \
  --scanner skill-vetter
```

`certify` 会依次：

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

### 4. 查看可读状态

```bash
agent-sec-cli skill-ledger status /path/to/your-skill
```

### 5. 审计完整版本链

深度验证全部历史版本——校验哈希完整性、签名有效性和版本链链接：

```bash
agent-sec-cli skill-ledger audit /path/to/your-skill

# 同时验证快照文件哈希
agent-sec-cli skill-ledger audit /path/to/your-skill --verify-snapshots
```

### 6. Agent 驱动的完整扫描（推荐方式）

最强大的使用方式是通过 AI Agent 自然语言触发。Agent 会自动编排 Phase 0 → 1 → 2 全流程：

| 说法 | 效果 |
|------|------|
| "扫描 /path/to/skill" | 对指定 Skill 执行完整扫描 |
| "扫描所有 skill" | 批量扫描 `config.json` 中配置的所有 Skill |
| "检查 skill 状态" | 仅输出状态分诊表，不执行扫描 |

三阶段工作流：

- **Phase 0**（环境准备）：校验 CLI、密钥、自身完整性，解析目标 Skill，输出分诊表
- **Phase 1**（安全扫描）：`skill-vetter` 四阶段审查——来源验证 → 代码审查 → 权限边界评估 → 风险分级
- **Phase 2**（建版签名）：调用 `certify` 将扫描结果写入版本链并签名

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
│  │   check / certify / audit / status        │     │
│  └──────────────────────────────────────────┘     │
│                      │                            │
│                      ▼                            │
│           .skill-meta/latest.json                 │
│           (SignedManifest + 版本链)                 │
└───────────────────────────────────────────────────┘
```

- **第一层——自动 Hook（实时守卫）**：
  - **OpenClaw**：插件拦截所有对 `SKILL.md` 的 `read_file` 调用，在 Skill 加载前自动运行 `check`。
  - **copilot-shell**：Python hook 脚本（`cosh_hooks/skill_ledger_hook.py`）通过 `PreToolUse` 事件在 Skill 调用前自动运行 `check`。
  - 两者均对非 `pass` 状态输出警告。**零配置、始终启用。**
- **第二层——Agent 驱动扫描（深度审计）**：`skill-ledger` Skill 驱动完整的四阶段安全扫描并生成签名认证。**按需触发**，由用户请求发起。

### 第一层：自动 Hook 防护（零配置）

**工作原理：**

OpenClaw 安全插件注册了一个 `before_tool_call` hook（优先级 80）。当 Agent 调用 `read_file` 读取任何 `SKILL.md` 文件时：

1. Hook 从文件路径提取 Skill 目录
2. 确保签名密钥存在（缺失时自动初始化）
3. 执行 `agent-sec-cli skill-ledger check <skill_dir>`
4. 根据状态输出日志：

| 状态 | 日志输出 |
|------|---------|
| `pass` | `✅ pass — 'skill-name'` |
| `none` | `⚠️ Skill 'skill-name' has not been security-scanned yet` |
| `drifted` | `⚠️ Skill 'skill-name' content has changed since last scan` |
| `warn` | `⚠️ Skill 'skill-name' has low-risk findings — review recommended` |
| `deny` | `🚨 Skill 'skill-name' has high-risk findings — immediate review recommended` |
| `tampered` | `🚨 Skill 'skill-name' metadata signature verification failed` |

**设计原则：fail-open**——Hook 仅发出警告，永不阻断 Skill 加载，确保 Agent 可用性不受 CLI 错误或密钥缺失影响。

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

默认已包含三个内置目录：`~/.openclaw/skills/*`、`~/.copilot-shell/skills/*`、`/usr/share/anolisa/skills/*`。如需添加额外目录，创建或编辑 `~/.config/skill-ledger/config.json`：

```json
{
  "skillDirs": [
    "~/.copilot-shell/skills/*",
    "/opt/custom-skills/my-skill"
  ]
}
```

用户配置中的 `skillDirs` 会**追加**到默认目录之后（自动去重），无需重复声明默认目录。

- `"path/*"` — glob 模式：每个包含 `SKILL.md` 的子目录视为一个 Skill
- `"path/to/skill"` — 单个 Skill 目录（同样需包含 `SKILL.md`）

不存在的目录会被静默忽略。此外，对 Skill 执行 `check` 或 `certify` 时，未收录的目录会自动追加到配置中，方便后续 `--all` 批量操作。

#### 触发扫描

通过自然语言向 Agent 发出指令即可。Agent 自动执行完整 Phase 0 → 1 → 2 流程。

**Phase 1 安全扫描规则表（skill-vetter）：**

| 级别 | 规则 ID | 检测目标 |
|------|---------|---------|
| deny | `dangerous-exec` | 危险进程执行（`child_process`、`subprocess`） |
| deny | `dynamic-code-eval` | 动态代码执行（`eval()`、`new Function()`） |
| deny | `env-harvesting` | 环境变量批量采集 + 网络发送 |
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
| `agent-sec-cli skill-ledger init-keys` | 生成签名密钥对 |
| `agent-sec-cli skill-ledger check <dir>` | 检查完整性状态（JSON 输出） |
| `agent-sec-cli skill-ledger certify <dir> --findings <file>` | 将扫描结果签名写入 manifest |
| `agent-sec-cli skill-ledger status <dir>` | 可读状态展示 |
| `agent-sec-cli skill-ledger audit <dir>` | 深度验证版本链 |
| `agent-sec-cli skill-ledger list-scanners` | 查看已注册的扫描器列表 |
| `agent-sec-cli skill-ledger init-keys --force` | 轮换密钥（归档旧密钥） |

## 关键路径

| 路径 | 用途 |
|------|------|
| `~/.local/share/skill-ledger/key.enc` | 加密私钥 |
| `~/.local/share/skill-ledger/key.pub` | 公钥 |
| `~/.local/share/skill-ledger/keyring/` | 归档的历史公钥（密钥轮换后） |
| `~/.config/skill-ledger/config.json` | 配置文件（skillDirs、scanners） |
| `<skill_dir>/.skill-meta/latest.json` | 当前签名 manifest |
| `<skill_dir>/.skill-meta/versions/` | 版本链历史 |
