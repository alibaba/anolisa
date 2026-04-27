# Skill 安全技术方案（skill-ledger）

## 背景与目标

### 问题

AI Agent 通过加载 Skill（结构化指令 + 辅助脚本）扩展能力。Skill 来源多样（官方内置、社区分发、用户自建），可指示 Agent 执行 shell 命令、读写文件等高权限操作。当前缺乏对 Skill 内容完整性和安全性的系统化验证机制——恶意或被篡改的 Skill 可静默获取 Agent 的全部工具权限。

### 设计目标

1. **防篡改**：通过密码学签名的版本链（SignedManifest）保护 Skill 元数据，使篡改可被检测
2. **安全扫描集成**：提供可扩展的扫描器框架，支持 Agent 驱动（skill-vetter）和 CLI 自动调用两种模式
3. **实时守卫**：在 Skill 加载时自动执行完整性检查（hook 层），对异常状态输出告警
4. **零阻断**：所有检查采用 fail-open 策略——仅告警不阻断，确保 Agent 可用性

### 非目标

- 不替代操作系统级沙箱或进程隔离
- 不实现运行时行为监控（仅静态内容检查 + 签名验证）
- 当前版本不阻断 Skill 执行（后续可升级为可配置阻断）

---

## 1. 整体架构

```
┌───────────────────────────────────────────────────────┐
│              宿主系统 (OpenClaw / copilot-shell)        │
│                                                       │
│  ┌──────────────┐       ┌──────────────────────────┐ │
│  │  Hook 层      │       │  Agent 工作区              │ │
│  │  (门禁)       │       │                          │ │
│  │               │       │  ┌──────────────────┐   │ │
│  │ skill-ledger  │       │  │  skill-ledger    │   │ │
│  │  check (CLI)  │       │  │  (Skill)         │   │ │
│  │               │       │  │                  │   │ │
│  │ 读 latest.json│       │  │  Phase 1: vetter │   │ │
│  │ 验签名       │       │  │  → Agent 扫描     │   │ │
│  │ 比 fileHashes │       │  │                  │   │ │
│  │ 查 scanStatus  │       │  │  Phase 2: ledger │   │ │
│  │       │       │       │  │  → CLI 建版签名   │   │ │
│  │       ▼       │       │  └──────────────────┘   │ │
│  │ allow / 告警  │       │                          │ │
│  └───────────────┘       └──────────────────────────┘ │
│          │                          │                  │
│          └──── .skill-meta/ ────────┘                  │
│                                                       │
│  ~/.local/share/agent-sec/skill-ledger/                     │
│    key.enc (私钥)     ← certify / check(首次建版) 签名   │
│    key.pub (公钥)     ← check 验签                     │
└───────────────────────────────────────────────────────┘
```

**组件职责**：

- **skill-ledger CLI**：核心基础设施。提供 `check`（hook 调用，读 JSON + 验签 + 比哈希 + 输出状态）、`certify`（建版签名：接收外部 findings 或自动调用已注册扫描器，归一化结果后更新 manifest 并签名）、`init-keys`（生成签名密钥对）等子命令。所有 manifest 均经 Ed25519 数字签名保护，防止篡改。确定性逻辑，不依赖 LLM，不可被 prompt injection 绕过。
- **Scanner Registry**：可扩展扫描框架。通过配置注册扫描器（`builtin`/`cli`/`skill`/`api` 四种调用类型）和结果解析器（将异构扫描输出归一化为统一 `NormalizedFinding` 格式）。本版本仅实现 skill-vetter（`type: "skill"`，`parser: "findings-array"`），由 Agent 层驱动后通过 `certify` 消费结果。其余扫描器类型（`builtin` 内置规则扫描、`cli` 外部工具、`api` 远端服务）及对应 parser 为预留扩展点，后续按需实现。
- **skill-ledger Skill**：一个 Skill，两个阶段。Phase 1（vetter）指导 Agent 按安全协议逐文件扫描并输出 findings；Phase 2（ledger）指导 Agent 调用 `skill-ledger certify` CLI 将 findings 写入版本链。必须先完成 Phase 1 再进入 Phase 2。
- **Hook 层**：门禁。调用 `skill-ledger check`，根据返回状态决定放行或输出告警日志。非 `pass` 状态时仅告警提示，不阻断 Skill 执行。

---

## 2. 数据模型与安全架构

### 目录结构

```
<skill_dir>/
├── ...                        # Skill 文件（不修改）
└── .skill-meta/
    ├── latest.json            # 最新 SignedManifest（含数字签名）
    ├── versions/
    │   ├── v000001.json       # 首版 manifest（含数字签名）
    │   ├── v000001.snapshot/  # 首版文件快照
    │   ├── v000002.json
    │   ├── v000002.snapshot/
    │   └── ...

~/.config/agent-sec/skill-ledger/         # 用户配置（XDG_CONFIG_HOME）
└── config.json                # 签名后端、skill 目录等偏好设置

~/.local/share/agent-sec/skill-ledger/   # 签名密钥存储（XDG_DATA_HOME，位于 skill 目录外部）
├── key.enc                    # Ed25519 私钥（默认明文存储，可选 AES-256-GCM 口令加密）
├── key.pub                    # Ed25519 公钥（明文，供验证使用）
└── keyring/                   # 可信公钥环（多机验证 / 密钥轮换）
    └── <fingerprint>.pub
```

**密钥与 skill 分离**：签名私钥存储在 `~/.local/share/agent-sec/skill-ledger/` 而非 `.skill-meta/` 内。即使攻击者完全控制 skill 目录，也无法伪造有效签名。密钥属于应用生成的数据（非用户手动编辑的配置），遵循 XDG Base Directory 规范放在 `$XDG_DATA_HOME` 下。

### SignedManifest 结构

```jsonc
{
  "version": 1,
  "versionId": "v000001",
  "previousVersionId": null,

  "skillName": "github",

  "fileHashes": {
    "SKILL.md": "sha256:b4e1...",
    "scripts/run.sh": "sha256:c5d2..."
  },

  "scans": [
    {
      "scanner": "skill-vetter",   // 扫描器标识
      "version": "0.1.0",          // 扫描器版本（可复现性）
      "status": "pass",            // 该扫描器结果：pass | warn | deny
      "findings": [],
      "scannedAt": "2026-04-13T10:00:00Z"
    }
    // 后续可扩展：{ "scanner": "license-checker", ... }
  ],
  "scanStatus": "pass",            // 聚合状态：none | pass | warn | deny（取最严重）

  "policy": "warning",             // 执行策略：warning（默认）| allow | block（预留扩展）

  "createdAt": "2026-04-13T10:00:05Z",
  "updatedAt": "2026-04-13T10:05:00Z",

  // ── 防篡改字段 ──────────────────────────────────────
  "manifestHash": "sha256:...",

  // 前一版本 manifest 的签名值（v000001 时为 null）。
  // 构成密码学版本链：篡改任何历史 manifest 将导致后续版本链断裂。
  "previousManifestSignature": null,

  // 对 manifestHash 的 Ed25519 数字签名。
  // 证明此 manifest 由持有签名私钥的 skill-ledger 实例创建。
  "signature": {
    "algorithm": "ed25519",      // 或 "gpg"（可插拔后端）
    "value": "<base64>",
    "keyFingerprint": "sha256:<fingerprint>"
  }
}
```

### 关键规则

**版本链**：当 skill 目录中文件发生变化（fileHashes 不匹配）时自动创建新版本。`latest.json` 始终指向最新版本。每个 manifest 的 `previousManifestSignature` 引用前一版本的签名值，形成密码学链——篡改任何历史版本将导致链断裂。

**fileHashes**：遍历 skill_dir 所有文件（排除 `.skill-meta/`、`.git/`），逐文件 SHA-256，按相对路径为 key 存入 map。`check` 时重新计算并逐条比对，可精确报告哪些文件被添加、删除或修改。

**manifestHash**：对 manifest 中除 `manifestHash`、`signature` 之外的所有字段做 Canonical JSON 序列化（键排序、无多余空格），取 SHA-256。`signature` 是对 `manifestHash` 的数字签名。两层设计：`manifestHash` 用于快速一致性校验，`signature` 提供密码学防篡改保护。

### 防篡改签名架构

#### 威胁模型

| # | 威胁 | 场景 |
|---|------|------|
| T1 | Skill 自我漂白 | 恶意 Skill 通过 shell 命令覆写自身 `.skill-meta/`，伪装为 pass |
| T2 | Agent 篡改历史 | 被劫持的 Agent 利用 shell 权限伪造整个 `.skill-meta/` 目录树 |
| T3 | 供应链更新攻击 | Skill 更新包中携带预制的 `.skill-meta/`，试图跳过扫描 |
| T4 | 降级攻击 | 用旧版 `latest.json` 替换当前版本，隐藏 deny 扫描结果 |

**防御原则**：签名权与文件访问权分离。签名私钥位于 `~/.local/share/agent-sec/skill-ledger/`（skill 目录外部），即使完全控制 skill 目录也无法伪造有效签名。

| 威胁 | 缓解措施 |
|------|---------|
| T1 | Skill 可写 `.skill-meta/` 但无签名私钥 → 签名验证失败 → `tampered` |
| T2 | 同上——Agent 无签名私钥（私钥位于 skill 目录外部，启用口令保护时更安全） |
| T3 | 外部预制的 `.skill-meta/` 密钥指纹不匹配本机 → `tampered` |
| T4 | `previousManifestSignature` 版本链 → 回滚 `latest.json` 导致链断裂 → `tampered` |

#### 可插拔签名后端

```python
class SigningBackend(Protocol):
    name: str
    def sign(self, data: bytes) -> tuple[str, str]: ...          # (signature, fingerprint)
    def verify(self, data: bytes, signature: str, fingerprint: str) -> bool: ...
    def get_public_key_fingerprint(self) -> str: ...
```

| 层级 | 后端 | 说明 |
|------|------|------|
| 默认（本版本实现） | **Ed25519Backend** | Python `cryptography` 库，零外部进程依赖，验签 ~0.1ms |
| 预留接口 | **GpgBackend** | 调用系统 GPG，适用于强制要求 GPG 密钥环管理的企业环境 |
| 预留接口 | **Pkcs11Backend** | TPM / YubiKey / HSM 硬件密钥 |

本版本仅实现 `Ed25519Backend`。`SigningBackend` 接口已定义，`GpgBackend` 和 `Pkcs11Backend` 预留扩展点，后续按需实现。

通过 `~/.config/agent-sec/skill-ledger/config.json` 配置：
```jsonc
{
  "signingBackend": "ed25519",  // 默认值；可选 "gpg"
  "skillDirs": [
    "~/.openclaw/skills/*",         // glob 匹配目录下所有 skill
    "~/.copilot-shell/skills/*",
    "/usr/share/anolisa/skills/*",
    "/opt/custom-skills/my-tool"    // 单个 skill 目录
  ],

  // ── 扫描器注册（详见 §3 扫描能力架构） ──
  "scanners": [
    {
      "name": "skill-vetter",      // 本版本唯一实现的扫描器
      "type": "skill",             // 声明式：由 Agent 层驱动，CLI 不直接调用
      "parser": "findings-array",
      "description": "LLM-driven 4-phase skill audit"
    }
    // 后续扩展示例（本版本不实现）：
    // { "name": "pattern-scanner", "type": "builtin", "enabled": true, "parser": "findings-array" }
    // { "name": "license-checker", "type": "cli", "command": "...", "parser": "license-checker" }
    // { "name": "cloud-scanner", "type": "api", "endpoint": "...", "parser": "cloud-scanner" }
  ],

  // ── 结果解析器注册 ──
  "parsers": {
    "findings-array": {            // 恒等解析器，输入已是标准格式（本版本唯一实现）
      "type": "findings-array"
    }
    // 后续扩展示例（本版本不实现）：
    // "license-checker": { "type": "field-mapping", "rootPath": "$.results", "mappings": {...}, "levelMap": {...} }
    // "sarif-parser": { "type": "sarif" }
    // "custom-parser": { "type": "custom", "entrypoint": "my_module:parse" }
  }
}
```

`skillDirs` 用于 `--all` 模式（如 `certify --all`），支持两种格式：
- **glob 模式**：`path/*` — 匹配目录下每个**包含 `SKILL.md`** 的子目录（如 `~/.openclaw/skills/*` 展开为 `github/`、`docker/` 等）
- **单目录**：直接指定一个 skill 目录路径（同样需包含 `SKILL.md` 才会被识别）

不存在的目录会被静默忽略。

**默认值**：内置三个默认目录（`~/.openclaw/skills/*`、`~/.copilot-shell/skills/*`、`/usr/share/anolisa/skills/*`），覆盖 OpenClaw、copilot-shell 和系统级 skill。

**合并策略**：用户配置中的 `skillDirs` 为**追加合并**（additive merge）——默认目录在前，用户目录在后，自动去重。用户无需重复声明默认目录。其余配置项（如 `signingBackend`）仍为覆盖合并。

**自动记忆**：用户对某个 skill 执行 `check` 或 `certify` 时，若该 skill 目录不在当前 `skillDirs` 中，会自动追加。若父目录下有 ≥2 个包含 `SKILL.md` 的兄弟 skill，则追加父目录 glob（`parent/*`）而非单个路径。追加后自动压缩（compact）：若某 glob 已覆盖某个单目录条目，则移除冗余的单目录条目。

#### 默认后端：Ed25519 + 加密密钥文件

**选择 Ed25519 而非 GPG 作为默认后端的理由**：

- **性能**：验签 ~0.1ms（进程内）vs GPG ~50–200ms（fork 进程 + 加载密钥环）。`check` 位于 hook 热路径，每次 Skill 调用均触发，100–1000× 的延迟差异不可接受。
- **零依赖**：Python `cryptography` 库提供 `Ed25519PrivateKey` / `Ed25519PublicKey`，无需安装 GPG。
- **跨平台一致**：不存在 `gpg` vs `gpg2` 二进制命名差异、`GNUPGHOME` 配置、`trustdb` 等平台问题。
- **代码简洁**：几行 `cryptography` API 调用 vs shell out + stderr 解析。

GPG 仍是**分发签名**（sign-skill.sh → trusted-keys → verifier.py）的正确选择。两个信任域各用各的工具：

| | sign-skill.sh（已有） | skill-ledger（新增） |
|---|---|---|
| 信任模型 | 发布者 → 终端用户 | 本机系统 → 自身 |
| 签名频率 | 每次发布 / 部署 | 每次 certify；每次 hook 验签 |
| 热路径 | 否（构建时） | **是**（PreToolUse hook） |
| 默认后端 | GPG（合理） | **Ed25519**（合理） |

#### 密钥管理

**密钥生成**（`skill-ledger init-keys`）：

```
1. 生成 Ed25519 密钥对（cryptography.hazmat.primitives.asymmetric.ed25519）
2. 若指定 --passphrase 或 SKILL_LEDGER_PASSPHRASE 环境变量：
   用 scrypt(passphrase, salt) 派生密钥 → AES-256-GCM 加密私钥
3. 否则：直接存储 32 字节原始种子（明文），依赖文件权限保护
4. 写入 ~/.local/share/agent-sec/skill-ledger/key.enc（mode 0600）
5. 写入公钥 → ~/.local/share/agent-sec/skill-ledger/key.pub
6. 输出公钥指纹 sha256:<hex>，以及 "encrypted": true/false
```

加密密钥文件格式（仅在指定口令时使用）：
```
┌─────────────────────────────────────────────────────┐
│  key.enc                                            │
├─────────────────────────────────────────────────────┤
│  salt       (16 bytes, random)                      │
│  iv         (12 bytes, random)                      │
│  authTag    (16 bytes, GCM authentication tag)      │
│  ciphertext (encrypted Ed25519 private key)         │
├─────────────────────────────────────────────────────┤
│  解密：                                              │
│  dk  = scrypt(passphrase, salt, N=2^17, r=8, p=1)  │
│  key = AES-256-GCM.decrypt(dk, iv, ciphertext, tag)│
└─────────────────────────────────────────────────────┘
```

**口令缓存**：若私钥已加密，首次签名时提示输入口令（或通过 `SKILL_LEDGER_PASSPHRASE` 环境变量提供），解密后在进程生命周期内缓存（类似 ssh-agent）。若私钥未加密则无需口令。`check`（验签）**仅需公钥**，无需口令——hook 热路径零交互。

---

## 3. skill-ledger CLI

### 子命令概览

| 子命令 | 用途 | 本版本状态 |
|--------|------|-----------|
| `init-keys` | 生成签名密钥对 | 已实现 |
| `check` | 状态检查（供 hook 调用） | 已实现 |
| `certify` | 建版签名（接收扫描结果） | 已实现 |
| `status` | 查询整体安全状况（系统级概览） | 已实现 |
| `list-scanners` | 列出已注册扫描器 | 已实现 |
| `audit` | 深度校验版本链完整性 | 已实现 |
| `rotate-keys` | 密钥轮换 | 预留接口 |
| `set-policy` | 设置执行策略 | 预留接口 |

### 子命令详述

**`skill-ledger init-keys [--force]`** — 生成签名密钥对

生成 Ed25519 密钥对，写入 `~/.local/share/agent-sec/skill-ledger/key.enc`（mode 0600）。默认不加密（明文种子）；指定 `--passphrase` 或设置 `SKILL_LEDGER_PASSPHRASE` 环境变量时使用 scrypt + AES-256-GCM 加密。输出公钥指纹。

**`skill-ledger rotate-keys`** — 密钥轮换（预留接口，本版本不实现）

设计思路：生成新密钥对 → 用新密钥重签 `latest.json` → 旧公钥移入 `keyring/` 供历史验证。

**`skill-ledger check <skill_dir>`** — 供 hook 调用的状态检查

判定流程（按优先级）：

1. **无 manifest** → 自动建版（`scanStatus: "none"`，签名写入 `latest.json`）→ 返回 `none`
2. **fileHashes 不匹配** → 返回 `drifted`（附 added/removed/modified 详情）
3. **签名验证失败** → 返回 `tampered`
4. **签名有效** → 按 `scanStatus` 返回 `deny` / `warn` / `none` / `pass`

输出为单行 JSON，hook 直接解析。首次建版需私钥签名，后续验签仅需公钥。

> **关键设计：fileHashes 先于签名验证。** 文件已变更时无论签名有效与否均为 `drifted`。`tampered` 仅在内容未变但 manifest 被伪造时触发（如 `scanStatus` 被篡改），是真正的元数据安全事件。

**`skill-ledger certify <skill_dir> [--findings <findings.json>] [--scanner <name>] [--scanner-version <ver>] [--scanners <name,...>]`** — 建版签名

**`skill-ledger certify --all [--findings <findings.json>] [--scanner <name>] [--scanner-version <ver>] [--scanners <name,...>]`** — 批量建版签名

两种输入模式：

- **外部提供模式**（`--findings`）：读取已有的 findings 文件（如 Agent/skill-vetter 产出的扫描结果）。`--scanner` 指定扫描器名称（默认 `"skill-vetter"`），用于 parser 查找和 ScanEntry 构建。
- **自动调用模式**（无 `--findings`）：从 `config.json` 加载已注册扫描器，自动调用非 `skill` 类型的扫描器并收集结果。`--scanners` 可限定调用范围。

> **本版本实现范围**：仅注册 skill-vetter（`type: "skill"`），自动调用模式跳过 `skill` 类型扫描器，因此当前仅外部提供模式可用。框架已就绪，待后续注册 `builtin`/`cli`/`api` 类型扫描器后，自动调用模式即可生效。

`--all` 模式从 `skillDirs` 配置解析所有 skill 目录，逐一执行建版签名。

三阶段流程：

| 阶段 | 职责 | 关键行为 |
|------|------|---------|
| **一：对齐** | 确保 manifest 与磁盘文件一致 | 无 manifest 或 fileHashes 不匹配时先建版（递增 versionId、创建 snapshot、签名写入 latest.json） |
| **二：收集** | 获取扫描结果 | `--findings` 模式读取外部文件；自动调用模式逐个触发非 `skill` 类型扫描器，输出经 parser 归一化为 `NormalizedFinding[]` |
| **三：签名** | 更新 manifest 并签名 | 合并 scan 条目 → 聚合 `scanStatus`（取最严重级别）→ 重算 `manifestHash` → Ed25519 签名 → 原子写入 |

**`skill-ledger set-policy <skill_dir> --policy <allow|block|warning>`** — 设置 skill 执行策略（预留接口）

用户对 skill 执行策略的管理入口。修改 manifest 中的 `policy` 字段，决定 hook 层对该 skill 的行为：
- `allow`：静默放行，不输出告警
- `block`：阻断执行（未来实现）
- `warning`：默认行为，放行 + 告警

**本版本仅预留 CLI 接口，内部不做实现。** 调用时输出提示信息并退出。

**`skill-ledger status [--verbose]`** — 查询整体安全状况（系统级概览）

返回 skill-ledger 系统的整体健康状态，包含三个区块：
- `keys`：签名密钥基础设施状态（是否已初始化、指纹、是否加密、归档密钥数量）
- `config`：配置摘要（skillDirs 模式数、已注册扫描器列表）
- `skills`：聚合健康度（已发现 Skill 数量、各状态计数、整体 `health` 标签：`healthy` / `unscanned` / `attention` / `critical` / `empty`）

使用 `--verbose` 时额外输出 `results` 数组，包含每个已注册 Skill 的详细检查结果。与 `check` 的定位区分：`check` 是单个 Skill 的完整性门禁（供 hook/plugin 调用，退出码语义化），`status` 是系统级态势感知（始终退出码 0，纯信息输出）。

**`skill-ledger list-scanners`** — 查看已注册扫描器

列出内置默认及 `~/.config/agent-sec/skill-ledger/config.json` 中注册的所有扫描器，包括名称、调用类型、结果解析器和启用状态。用于发现 `certify --scanner` 可用的扫描器名称。

**`skill-ledger audit <skill_dir>`** — 深度校验版本链完整性

遍历 `versions/` 逐版本验证 manifestHash、签名、`previousManifestSignature` 链接完整性。可选 `--verify-snapshots` 校验快照文件哈希。输出结构化校验结果。

### 扫描能力架构

#### 核心设计：调用与解析分离

扫描能力的核心洞察：**扫描器的调用方式**（如何触发）与**结果的解析方式**（如何归一化）是两个独立关注点。一个 `cli` 扫描器可能输出 SARIF 格式，一个 `skill` 扫描器可能输出 `findings-array` 格式。adapter 与 parser 独立选择。

> **本版本实现范围**：仅实现 skill-vetter（`type: "skill"` + `parser: "findings-array"`）。`builtin`/`cli`/`api` 类型的 Scanner Adapter、`sarif`/`field-mapping`/`custom` 类型的 Result Parser 均为预留架构设计，后续按需实现。

```
┌─────────────────────┐     ┌─────────────────────┐
│  Scanner Adapter     │     │  Result Parser       │
│  (how to invoke)     │     │  (how to normalize)  │
│                      │     │                      │
│  builtin             │     │  findings-array      │
│  cli                 │     │  sarif               │
│  skill               │     │  field-mapping       │
│  api                 │     │  custom              │
└──────────┬──────────┘     └──────────┬──────────┘
           │ raw output                 │
           └────────────┬───────────────┘
                        ▼
              NormalizedFinding[]
              → ScanEntry.findings
              → ScanEntry.status
              → aggregate → scanStatus
```

#### Scanner Adapter（调用类型）

| 类型 | 调用方式 | 输出捕获 | 适用场景 |
|---|---|---|---|
| **`builtin`** | 进程内 Python 调用，仅用标准库 | 函数返回值 | 始终可用，无 LLM、无网络依赖 |
| **`cli`** | 子进程调用（`command` 模板） | stdout / 输出文件 | 本地已安装的外部扫描工具 |
| **`skill`** | CLI 不直接调用——由 Agent 层编排 | 用户/Agent 提供结果文件路径 | skill-ledger 以 Skill 形式运行；或手动指定其它 Skill 扫描结果 |
| **`api`** | HTTP POST 至 `endpoint` | 响应体 | 远端扫描服务 |

**`skill` 类型的关键约束**：skill-ledger CLI 不能直接调用 Skill（Skill 需要 Agent/LLM）。因此 `type: skill` 是**声明式**的：

- 声明"扫描器 X 是一个 Skill，其输出格式为 Y"
- `certify` 的自动调用模式跳过 `skill` 类型扫描器
- `certify --findings <file> --scanner <name>` 在 Agent/用户手动执行后接收其输出
- 当 skill-ledger 自身作为 Skill 运行时，SKILL.md 在 Agent 层编排 `skill` 类型扫描器的调用

这保证 CLI 始终确定性运行，同时声明性地支持 Skill 执行模型。

#### NormalizedFinding（归一化合约）

所有扫描器的输出最终归一化为统一的 `NormalizedFinding` 结构，作为 `ScanEntry.findings` 的通用格式：

```jsonc
{
  "rule": "dangerous-exec",      // 规则/检查 ID
  "level": "deny",           // "deny" | "warn" | "pass"
  "message": "child_process exec detected in line 42",
  "file": "scripts/run.sh",     // 可选：受影响的文件路径
  "line": 42,                    // 可选：行号
  "metadata": {}                 // 可选：扫描器特定的额外数据
}
```

`level` 值域严格限定为 `deny | warn | pass`，与 `scanStatus` 聚合逻辑对齐。

#### Result Parser（结果解析器）

每个扫描器在 `config.json` 中声明其 `parser`，parser 负责将原始输出转换为 `NormalizedFinding[]`。

| 解析器类型 | 工作方式 | 适用场景 |
|---|---|---|
| **`findings-array`** | 恒等变换——输入已是 `[{rule, level, message, ...}]` | skill-vetter、pattern-scanner 及任何符合标准格式的扫描器 |
| **`sarif`** | 读取 SARIF v2.1 JSON，映射 `results[].level` → `level`，`results[].ruleId` → `rule` | 工业标准静态分析工具 |
| **`field-mapping`** | 声明式：用户定义 JSONPath 映射，从扫描器字段映射到 NormalizedFinding 字段 | 输出 JSON 但字段名不同的简单扫描器 |
| **`custom`** | 用户提供 Python 可调用对象（入口点或模块路径） | 无法声明式映射的复杂/私有格式 |

**Level 映射**：解析器通过 `levelMap` 将扫描器原生的严重级别映射到 `deny | warn | pass`：

```jsonc
"levelMap": {
  "error": "deny",
  "high": "deny",
  "medium": "warn",
  "warning": "warn",
  "low": "pass",
  "info": "pass"
}
```

#### 内置 pattern-scanner（预留，本版本不实现）

预留的**基线扫描器**设计——无 LLM、无网络、无外部工具依赖：

- **纯标准库**：`re`、`ast`、`pathlib`、`json`
- **规则驱动**：规则从 JSON 文件加载（可独立于代码更新）
- **覆盖范围**：实现 §4 Phase 1 规则表中的全部检测项：
  - 代码规则：`dangerous-exec`、`dynamic-code-eval`、`env-harvesting`、`crypto-mining`、`obfuscated-code`、`suspicious-network`、`exfiltration-pattern`
  - Prompt 文档规则：`prompt-override`、`hidden-instruction`、`unrestricted-tool-use`、`external-fetch-exec`、`privilege-escalation`
- **输出**：`findings-array` 格式（无需额外 parser）
- **定位**：不替代 LLM 扫描——捕获明显模式。LLM 驱动的 skill-vetter 处理语义/上下文威胁

> 本版本不实现。后续作为 `type: "builtin"` 扫描器注册后，`certify` 的自动调用模式即可在无 LLM 环境下自动执行静态规则检测。

#### Parser 查找逻辑

`certify` 阶段二根据 `--scanner` 名称在 `scanners[]` → `parsers{}` 中查找对应 parser，执行归一化。未注册的 scanner 回退到 `findings-array`（向后兼容）。

#### 设计原则

1. **Ledger ≠ Scanner** — skill-ledger 追踪完整性并签名 manifest。扫描是输入而非核心职责。但 `certify` 是**编排者**，知道哪些扫描器存在以及如何调用（自动调用模式）或如何解析其输出（外部提供模式）。

2. **Parser 作为归一化层** — 通用合约是 `NormalizedFinding`，而非原始扫描器格式。这使异构扫描器可组合。

3. **`skill` 类型是声明式的** — CLI 不调用 Skill；仅声明其存在，使 `certify` 知道使用哪个 parser 处理其输出。Agent 层编排不在 CLI 职责范围内。

4. **优雅降级** — 若无 parser 匹配，回退到 `findings-array`。后续实现 `builtin` pattern-scanner 后，`certify` 可在无外部 findings 时自动执行内置规则检测。

5. **独立发布周期** — 扫描器和解析器通过配置注册，非代码内嵌（`builtin` 除外）。新增扫描器 = 编辑 config.json，无需发布新版 skill-ledger。

---

## 4. skill-ledger Skill（vetter + ledger 两阶段）

### Skill 结构

```
skill-ledger/
  SKILL.md       # 包含 Phase 1 (vetter) 和 Phase 2 (ledger) 的完整指令
```

### Phase 1：安全扫描（vetter）

Agent 调用此 Skill 后，按 SKILL.md 指令使用 read/grep/shell tool 逐文件审查目标 Skill，参照 [skill-vetter 协议](https://github.com/openclaw/skills/blob/main/skills/spclaudehome/skill-vetter/SKILL.md)的四阶段框架：

1. **来源验证**：检查 Skill 来源（本地/远程/extension）、是否有 README/LICENSE
2. **强制代码审查**：逐文件扫描危险模式（下文规则表）
3. **权限边界评估**：Skill 声明的 `allowedTools` 与实际内容是否对齐
4. **风险分级**：汇总 findings，输出结构化 JSON

> **与 Scanner Registry 的关系**：skill-vetter 在 `config.json` 中注册为 `type: "skill"` 扫描器（见 §3 扫描能力架构），是本版本唯一实现的扫描器。Phase 1 即为 Agent 层编排 `skill` 类型扫描器的标准流程。其输出通过 `findings-array` parser 归一化为 `NormalizedFinding[]`，确保与 `certify` 的聚合逻辑对齐。后续将实现内置 `pattern-scanner` 覆盖同一规则表的静态检测子集，作为无 LLM 环境下的降级替代，届时 `certify` 的自动调用模式可直接触发。

**代码文件规则**（.js/.ts/.sh/.py 等）：

| 规则 ID | 级别 | 检测目标 |
|---------|------|---------|
| `dangerous-exec` | deny | child_process exec/spawn、subprocess |
| `dynamic-code-eval` | deny | eval()、new Function() |
| `env-harvesting` | deny | process.env 批量读取 + 网络发送 |
| `credential-access` | deny | 凭据与敏感文件访问（`~/.ssh/`、`.env`） |
| `system-modification` | deny | 系统文件篡改（`/etc/`、crontab） |
| `crypto-mining` | deny | stratum/coinhive/xmrig 特征 |
| `obfuscated-code` | warn | hex/base64 编码 + decode |
| `suspicious-network` | warn | 非标准端口、直连 IP |
| `exfiltration-pattern` | warn | 文件读取 + 网络发送组合 |
| `agent-data-access` | warn | Agent 身份数据访问（`MEMORY.md` 等） |
| `unauthorized-install` | warn | 未声明的包安装 |

**Prompt 文档规则**（.md 文件）：

| 规则 ID | 级别 | 检测目标 |
|---------|------|---------|
| `prompt-override` | deny | "ignore previous instructions" 等覆盖指令 |
| `hidden-instruction` | deny | 零宽字符、注释伪装隐藏指令 |
| `unrestricted-tool-use` | warn | 引导无约束 shell 执行 |
| `external-fetch-exec` | warn | 引导下载并执行外部内容 |
| `privilege-escalation` | warn | 引导 sudo、修改系统文件 |

Phase 1 输出：Agent 将 findings 写入临时文件（如 `/tmp/skill-vetter-findings-<skill_name>.json`）。

### Phase 2：建版签名（ledger）

SKILL.md 指令要求 Agent 在 Phase 1 完成后（且仅在完成后），调用 CLI 执行建版：

```bash
skill-ledger certify <skill_dir> --findings /tmp/skill-vetter-findings-<skill_name>.json --scanner skill-vetter
```

Phase 2 不能独立执行——SKILL.md 中明确约束"必须先完成 Phase 1 扫描并确认 findings 后才能进入 Phase 2"。CLI 的 `certify` 命令也会校验 findings.json 的存在和完整性。

---

## 5. Hook 告警策略

### 设计原则

为简化实现、减少对用户的干扰，当 hook 层（`skill-ledger check`）检测到非 `pass` 状态时，**仅输出告警信息，不阻断 Skill 执行**。告警信息通过宿主系统的日志/消息通道呈现给用户，用户可事后选择手动调用 skill-ledger Skill 进行扫描建版。

### 各状态的行为

| 状态 | 行为 | 告警内容 |
|------|------|---------|
| `pass` | 静默放行 | 无 |
| `warn` | 放行 + 告警 | `⚠️ Skill '<name>' 存在低风险项，建议关注` |
| `drifted` | 放行 + 告警 | `⚠️ Skill '<name>' 内容已变更，尚未重新扫描` |
| `none` | 放行 + 告警 | `⚠️ Skill '<name>' 尚未经过安全扫描` |
| `deny` | 放行 + 告警 | `🚨 Skill '<name>' 上次扫描存在高危项，请尽快处理` |
| `tampered` | 放行 + 告警 | `🚨 Skill '<name>' 元数据签名校验失败，建议重新扫描建版` |

所有非 `pass` 状态均**仅告警、不阻断**。`tampered` 触发条件较窄（内容未变但 manifest 被伪造），属于元数据可信度问题而非紧急安全事件，告警提示用户重新执行扫描建版即可恢复正常。

所有告警均通过宿主系统日志/消息通道输出，保证可追溯。

### 后续升级路径

当前的告警模式为最小可用版本。后续可按需升级：对 `deny` 状态改为阻断 + 用户选择，对 `drifted`/`none` 状态可配置为自动触发扫描建版。升级时仅需修改 hook handler 的返回值，不影响 CLI 和 Skill 侧逻辑。

### 向后兼容

若 `check` 遇到无签名的 `.skill-meta/`（升级前遗留数据），视为 `none` 而非 `tampered`。首次执行 `certify` 后将自动补签。

---

## 6. 宿主集成

skill-ledger 需适配两个宿主系统，两者 Skill 模型和 Hook 机制存在本质差异：

| 维度 | OpenClaw | copilot-shell |
|------|---------|---------------|
| Skill 调用方式 | Agent 通过 read tool 读取 SKILL.md | Agent 调用 `Skill` tool，框架加载返回内容 |
| Hook 机制 | Plugin Hook（进程内 async handler） | Command Hook（fork 子进程，stdin/stdout JSON） |
| 告警输出 | `api.logger.warn` | `decision: "allow"` + `reason` 字段 |
| Skill 安装路径 | `~/.openclaw/skills/` | `~/.copilot-shell/skills/` |

两个实现共享相同的语义：拦截 Skill 加载 → 调用 `skill-ledger check` → 非 `pass` 时告警但不阻断。

### 6.1 OpenClaw（Plugin Hook）

以 OpenClaw Plugin 形式分发。`before_tool_call` handler 过滤 read tool 对 `*/SKILL.md` 的访问，解析 `skill_dir` 后调用 `agent-sec-cli skill-ledger check`。告警通过 `api.logger.warn` 输出。

### 6.2 copilot-shell（Command Hook）

独立 Python 脚本 `cosh-extension/hooks/skill_ledger_hook.py`，专为 stdin/stdout 协议设计，不依赖 `agent_sec_cli` 包。

配置：
```jsonc
// ~/.copilot-shell/settings.json
{
  "hooks": {
    "PreToolUse": [{
      "matcher": "skill",
      "hooks": [{
        "type": "command",
        "name": "skill-ledger",
        "command": "python3 cosh-extension/hooks/skill_ledger_hook.py",
        "timeout": 10000
      }]
    }]
  }
}
```

**Skill 目录定位**：`tool_input` 仅含 skill 名称，hook 脚本按 project → custom → user → extension → system 优先级自行查找。project 级路径通过 event 的 `cwd` 字段推断。

**extension Skills**：读取 `~/.copilot-shell/extensions/<ext>/` 下的 `cosh-extension.json` 配置，按 `skills` 字段确定 skill 基目录，支持 `link` 类型安装（跟随 `.qwen-extension-install.json` 中的 `source` 路径）。extension skill 与其他级别 skill 享有相同的安全检查。

**remote Skills**：首次下载的 remote skill 无 `.skill-meta/`，hook 返回 `unscanned`，输出告警但不阻断。
