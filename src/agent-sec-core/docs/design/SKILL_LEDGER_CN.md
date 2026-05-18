# Skill 安全技术方案（skill-ledger）

## 背景与目标

### 问题

AI Agent 通过加载 Skill（结构化指令 + 辅助脚本）扩展能力。Skill 来源多样（官方内置、社区分发、用户自建），可指示 Agent 执行 shell 命令、读写文件等高权限操作。当前缺乏对 Skill 内容完整性和安全性的系统化验证机制——恶意或被篡改的 Skill 可静默获取 Agent 的全部工具权限。

### 设计目标

1. **防篡改**：通过密码学签名的版本链（SignedManifest）保护 Skill 元数据，使篡改可被检测
2. **安全扫描集成**：提供可扩展的扫描器框架，支持 Agent 驱动（skill-vetter）和 CLI 自动调用两种模式
3. **实时守卫**：在 Skill 加载时自动执行完整性检查（hook 层），对异常状态输出告警或要求用户确认
4. **可用性优先**：CLI 异常、超时、输出不可解析时保持 fail-open；检查成功后按状态分级处理

### 非目标

- 不替代操作系统级沙箱或进程隔离
- 不实现运行时行为监控（仅静态内容检查 + 签名验证）
- 不提供用户自定义的 hook 执行策略；当前版本采用统一默认策略

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
│  │ 读 latest.json│       │  │  Phase 1: 状态   │   │ │
│  │ 验签名       │       │  │  Phase 2: 快扫   │   │ │
│  │ 比 fileHashes │       │  │  Phase 3: 深扫   │   │ │
│  │ 查 scanStatus  │       │  │  scan/certify 签名 │   │ │
│  │       │       │       │  │                  │   │ │
│  │       ▼       │       │  └──────────────────┘   │ │
│  │ allow / 告警 / 确认 │  │                          │ │
│  └───────────────┘       └──────────────────────────┘ │
│          │                          │                  │
│          └──── .skill-meta/ ────────┘                  │
│                                                       │
│  ~/.local/share/agent-sec/skill-ledger/                     │
│    key.enc (私钥)     ← scan/certify 签名                   │
│    check 首次无 manifest → 未签名 baseline（无需私钥）       │
│    key.pub (公钥)     ← check 验签                     │
└───────────────────────────────────────────────────────┘
```

**组件职责**：

- **skill-ledger CLI**：核心基础设施。提供 `init`（初始化密钥并可为已覆盖 Skill 建立快速扫描 baseline）、`scan`（运行内置快速扫描器并签名入账）、`check`（hook 调用，读 JSON + 验签 + 比哈希 + 输出状态）、`certify`（导入外部 findings 并签名）等子命令。`scan` / `certify` 写入的 manifest 经 Ed25519 数字签名保护，防止篡改；`check` 在无 manifest 时只创建未签名 baseline，用于后续 drift 检测。确定性逻辑不依赖 LLM，不可被 prompt injection 绕过。
- **Scanner Registry**：可扩展扫描框架。通过配置注册扫描器（`builtin`/`cli`/`skill`/`api` 四种调用类型）和结果解析器（将异构扫描输出归一化为统一 `NormalizedFinding` 格式）。本版本默认注册 `skill-vetter`（`type: "skill"`，由 Agent 深度扫描后通过 `certify --findings` 消费）、`code-scanner` 和 `static-scanner`（均为 `type: "builtin"`，可由 `scan` 自动调用）。当前仅实现 `findings-array` parser；`cli`/`api` adapter 及其它 parser 类型为预留扩展点。旧名称 `skill-code-scanner`、`cisco-static-scanner` 仅作为兼容 alias 读取，不再作为公开名称展示或写入新 manifest。
- **skill-ledger Skill**：一个 Skill，三个阶段。Phase 1 做环境准备与状态查看；Phase 2 默认执行快速扫描认证（`scan` 调用内置 `code-scanner` 与 `static-scanner`）；Phase 3 在用户显式要求或确认后执行 Agent 驱动深度扫描（`skill-vetter`），再用 `certify --findings ... --delete-findings` 写入版本链。
- **Hook 层**：门禁。调用 `skill-ledger check`，根据返回状态决定静默放行、告警放行或要求用户确认。CLI 不可用、执行失败、超时或输出不可解析时保持 fail-open。

---

## 2. 数据模型与安全架构

### 目录结构

```
<skill_dir>/
├── ...                        # Skill 文件（不修改）
└── .skill-meta/
    ├── latest.json            # 最新 manifest（certify 后含数字签名）
    ├── versions/
    │   ├── v000001.json       # 首版 manifest（check baseline 可未签名）
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

  "policy": "warning",             // 预留字段：当前 hook 不读取，未来可扩展 allow | warning | block

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
    "algorithm": "ed25519",      // 当前固定 ed25519；其它后端预留
    "value": "<base64>",
    "keyFingerprint": "sha256:<fingerprint>"
  }
}
```

### 关键规则

**版本链**：当 skill 目录中文件发生变化（fileHashes 不匹配）时，`certify` 会创建新版本并签名。正常写入时 `latest.json` 指向最新版本。每个签名 manifest 的 `previousManifestSignature` 引用前一版本的签名值，形成密码学链；历史链完整性由 `audit` 深度校验。

**fileHashes**：遍历 skill_dir 文件（排除 `.skill-meta/`、`.git/`，跳过符号链接），逐文件 SHA-256，按相对路径为 key 存入 map。`check` 时重新计算并逐条比对，可精确报告哪些文件被添加、删除或修改。

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
| T4 | 当前 hook 热路径的 `check` 只校验 `latest.json` 本身，不遍历 `versions/`；回滚检测依赖 `audit`，会发现 `latest.json` 未指向最高版本或历史链断裂 |

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

本版本仅实现并启用 `Ed25519Backend`。`SigningBackend` 接口已定义，`GpgBackend` 和 `Pkcs11Backend` 仅是预留扩展点；当前 CLI backend 直接使用 `NativeEd25519Backend`，不会根据配置切换到 GPG 或硬件密钥。

通过 `~/.config/agent-sec/skill-ledger/config.json` 配置：
```jsonc
{
  "signingBackend": "ed25519",  // 当前实现固定使用 ed25519；该字段保留给未来扩展
  "enableDefaultSkillDirs": true,   // 默认 true；false 时仅使用 managedSkillDirs
  "managedSkillDirs": [
    "/opt/custom-skills/*",         // glob 匹配目录下所有 skill
    "/opt/custom-skills/my-tool"    // 单个 skill 目录
  ],

  // ── 扫描器注册（详见 §3 扫描能力架构） ──
  "scanners": [
    {
      "name": "skill-vetter",
      "type": "skill",             // 声明式：由 Agent 层驱动，CLI 不直接调用
      "parser": "findings-array",
      "description": "LLM-driven 4-phase skill audit"
    },
    {
      "name": "code-scanner",
      "type": "builtin",
      "parser": "findings-array",
      "enabled": true,
      "description": "Scan Skill code files via code-scanner"
    },
    {
      "name": "static-scanner",
      "type": "builtin",
      "parser": "findings-array",
      "enabled": true,
      "description": "Static Skill security scanner based on Cisco skill-scanner rules"
    }
    // 后续扩展示例（本版本不实现）：
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

有效 Skill 目录由内置默认目录和 `managedSkillDirs` 共同组成，用于 `init` baseline、`check --all` 和 `scan --all`。`managedSkillDirs` 支持两种格式：
- **glob 模式**：`path/*` — 匹配目录下每个**包含 `SKILL.md`** 的子目录（如 `~/.openclaw/skills/*` 展开为 `github/`、`docker/` 等）
- **单目录**：直接指定一个 skill 目录路径（同样需包含 `SKILL.md` 才会被识别）

不存在的目录会被静默忽略。

**默认值**：内置三个默认目录（`~/.openclaw/skills/*`、`~/.copilot-shell/skills/*`、`/usr/share/anolisa/skills/*`），覆盖 OpenClaw、copilot-shell 和系统级 skill。

**合并策略**：默认目录默认启用，由 `enableDefaultSkillDirs` 控制；`managedSkillDirs` 存放 skill-ledger 动态管理或用户额外配置的目录，不再兼容旧的 `skillDirs` 字段。解析时默认目录在前，`managedSkillDirs` 在后，自动去重。`scanners` 按 `name` 合并，用户配置可覆盖同名扫描器；`signingBackend` 当前会被读取到配置摘要中，但不会改变实际签名后端。

**自动记忆**：用户对某个 skill 执行 `check`、`scan` 或 `certify` 时，若该 skill 目录不在当前有效目录中，会自动追加到 `managedSkillDirs`。若父目录下有 ≥2 个包含 `SKILL.md` 的兄弟 skill，则追加父目录 glob（`parent/*`）而非单个路径。追加后自动压缩（compact）：若某 glob 已覆盖某个单目录条目，则移除冗余的单目录条目。

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

**密钥生成**（`skill-ledger init`，或兼容入口 `init-keys`）：

```
1. 生成 Ed25519 密钥对（cryptography.hazmat.primitives.asymmetric.ed25519）
2. 若指定 `--passphrase`，并通过交互输入口令或 `SKILL_LEDGER_PASSPHRASE` 环境变量提供口令：
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
│  ciphertext_with_tag                                │
│    = encrypted Ed25519 private key + 16-byte GCM tag│
├─────────────────────────────────────────────────────┤
│  解密：                                              │
│  dk  = scrypt(passphrase, salt, N=2^17, r=8, p=1)  │
│  key = AES-256-GCM.decrypt(                         │
│    dk, iv, ciphertext_with_tag)                      │
└─────────────────────────────────────────────────────┘
```

**口令缓存**：若私钥已加密，首次签名时提示输入口令（或通过 `SKILL_LEDGER_PASSPHRASE` 环境变量提供），解密后在进程生命周期内缓存（类似 ssh-agent）。若私钥未加密则无需口令。`check`（验签）**仅需公钥**，无需口令——hook 热路径零交互。

---

## 3. skill-ledger CLI

### 子命令概览

| 子命令 | 用途 | 本版本状态 |
|--------|------|-----------|
| `init` | 初始化密钥，并默认为已覆盖 Skill 建立快速扫描 baseline | 已实现 |
| `scan` | 运行内置快速扫描器并签名写入 manifest | 已实现 |
| `check` | 状态检查（供 hook 调用） | 已实现 |
| `certify` | 导入外部 findings 并签名写入 manifest | 已实现 |
| `status` | 查询整体安全状况（系统级概览） | 已实现 |
| `list-scanners` | 列出已注册扫描器 | 已实现 |
| `audit` | 深度校验版本链完整性 | 已实现 |

### 子命令详述

**`skill-ledger init [--no-baseline] [--passphrase]`** — 初始化 Skill Ledger

若密钥不存在，生成 Ed25519 密钥对并写入 `~/.local/share/agent-sec/skill-ledger/key.enc`（mode 0600）；若密钥已存在则复用，不轮换。默认不加密（明文种子）；只有指定 `--passphrase` 时才启用口令逻辑，此时可交互输入口令，或设置 `SKILL_LEDGER_PASSPHRASE` 环境变量用于非交互场景。

默认行为还会发现已覆盖目录中的 Skill，并执行补齐式快速扫描，建立签名 baseline。`--no-baseline` 只初始化密钥，不扫描 Skill。不访问、不可写或扫描失败的 Skill 会记录为 `error`/`skipped` 结果，不阻断其它 Skill。

兼容入口 `init-keys` 仍保留，但作为低层命令隐藏，不在普通 help 与用户主流程中展示。

**`skill-ledger rotate-keys`** — 密钥轮换（预留接口，本版本不实现）

设计思路：生成新密钥对 → 用新密钥重签 `latest.json` → 旧公钥移入 `keyring/` 供历史验证。

**`skill-ledger check <skill_dir>`** — 供 hook 调用的状态检查

判定流程（按优先级）：

1. **无 manifest** → 自动创建未签名 baseline（`scanStatus: "none"`，写入 `latest.json` 和首个 snapshot）→ 返回 `none`
2. **fileHashes 不匹配** → 返回 `drifted`（附 added/removed/modified 详情）
3. **签名验证失败** → 返回 `tampered`
4. **签名有效** → 按 `scanStatus` 返回 `deny` / `warn` / `none` / `pass`

输出为单行 JSON，hook 直接解析。首次 `check` 不需要私钥，也不会签名；后续已签名 manifest 的验签仅需公钥。

> **关键设计：fileHashes 先于签名验证。** 文件已变更时无论签名有效与否均为 `drifted`。`tampered` 仅在内容未变但 manifest 被伪造时触发（如 `scanStatus` 被篡改），是真正的元数据安全事件。

**`skill-ledger scan <skill_dir> [--force] [--scanners <name,...>]`** — 快速扫描并签名入账

**`skill-ledger scan --all [--force] [--scanners <name,...>]`** — 批量快速扫描

`scan` 是内置快速扫描器的主入口，不是 dry-run。默认 scanner 为 `code-scanner,static-scanner`；执行结束后自动更新 `manifest.scans[]`，聚合 `scanStatus`，重算 `manifestHash`，并写入 Ed25519 签名。

默认采用补齐式扫描：

- 无 manifest、无扫描结果、缺少部分默认 scanner 结果时，只运行缺失 scanner。
- `drifted` 时按当前文件创建新版本并运行请求的 scanner。
- `tampered` 时用户显式执行 `scan` 即表示按当前文件重新建立可信记录；CLI 忽略已损坏 manifest 的可信性，重新扫描并写入新的签名 manifest，最终状态只按本次扫描结果聚合为 `pass` / `warn` / `deny`。
- 已有对应 scanner 结果且文件未变时跳过该 scanner。

`scan --all` 对所有发现的 Skill 执行相同补齐逻辑；若没有任何 scanner 需要执行，不写 manifest，只报告 `noop`。`--force` 会强制重跑请求 scanner 并重签 manifest。

**`skill-ledger certify <skill_dir> --findings <findings.json> [--scanner <name>] [--scanner-version <ver>] [--delete-findings]`** — 导入外部 findings

`certify` 只负责导入外部 findings，主要服务 Agent/Skill 驱动的 `skill-vetter` 深度扫描。它必须传 `--findings`；若用户想运行内置快速扫描，应使用 `scan`。`--scanner` 指定扫描器名称（默认 `"skill-vetter"`），用于 parser 查找和 ScanEntry 构建。

若签名密钥尚未初始化，`certify` 会自动生成默认无口令 key，并在输出中标记 `keyCreated: true`。`--delete-findings` 仅在 findings 成功写入并签名后删除该文件；失败时保留，便于排查或重试。

导入流程：

| 阶段 | 职责 | 关键行为 |
|------|------|---------|
| **一：对齐** | 确保 manifest 与磁盘文件一致 | 无 manifest、drifted 或 tampered 时按当前文件创建新版本；首次 `check` 只创建未签名 baseline |
| **二：导入** | 获取扫描结果 | 读取外部 findings 文件，输出经 parser 归一化为 `NormalizedFinding[]` |
| **三：签名** | 更新 manifest 并签名 | 合并 scan 条目 → 聚合 `scanStatus`（取最严重级别）→ 重算 `manifestHash` → Ed25519 签名 → 原子写入 |

**`skill-ledger set-policy <skill_dir> --policy <allow|block|warning>`** — 设置 skill 执行策略（预留接口）

用户对 skill 执行策略的管理入口。当前 hook 不读取该字段，统一默认策略见第 5 节；以下语义仅作为未来可配置策略预留：
- `allow`：静默放行，不输出告警
- `block`：阻断执行
- `warning`：放行 + 告警

**本版本仅预留 CLI 接口，内部不做实现。** 调用时输出提示信息并退出，不改变当前 hook 默认策略。

**`skill-ledger status [--verbose]`** — 查询整体安全状况（系统级概览）

返回 skill-ledger 系统的整体健康状态，包含三个区块：
- `keys`：签名密钥基础设施状态（是否已初始化、指纹、是否加密、归档密钥数量）
- `config`：配置摘要（默认目录、managedSkillDirs 模式数、已注册扫描器列表）
- `skills`：聚合健康度（已发现 Skill 数量、各状态计数、整体 `health` 标签：`healthy` / `unscanned` / `attention` / `critical` / `empty`）

使用 `--verbose` 时额外输出 `results` 数组，包含每个已注册 Skill 的详细检查结果。与 `check` 的定位区分：`check` 是单个 Skill 的完整性门禁（供 hook/plugin 调用，退出码语义化），`status` 是系统级态势感知（始终退出码 0，纯信息输出）。

**`skill-ledger list-scanners`** — 查看已注册扫描器

列出内置默认及 `~/.config/agent-sec/skill-ledger/config.json` 中注册的扫描器，包括公开名称、调用类型、结果解析器、启用状态和 `autoInvocable`。默认只展示 canonical 名称：`code-scanner`、`static-scanner`、`skill-vetter`；旧名称只作为兼容 alias 读取。用于发现 `scan --scanners` 和 `certify --scanner` 可用的扫描器名称。

**`skill-ledger audit <skill_dir>`** — 深度校验版本链完整性

遍历 `versions/` 逐版本验证 manifestHash、签名、`previousManifestSignature` 链接完整性。可选 `--verify-snapshots` 校验快照文件哈希。输出结构化校验结果。

### 扫描能力架构

#### 核心设计：调用与解析分离

扫描能力的核心洞察：**扫描器的调用方式**（如何触发）与**结果的解析方式**（如何归一化）是两个独立关注点。一个 `cli` 扫描器可能输出 SARIF 格式，一个 `skill` 扫描器可能输出 `findings-array` 格式。adapter 与 parser 独立选择。

> **本版本实现范围**：已实现 `skill-vetter`（`type: "skill"` + `parser: "findings-array"`）、`code-scanner`（`type: "builtin"`）和 `static-scanner`（`type: "builtin"`）。`cli`/`api` 类型的 Scanner Adapter、`sarif`/`field-mapping`/`custom` 类型的 Result Parser 均为预留架构设计，后续按需实现。

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
| **`builtin`** | 进程内 Python 调用，由内置 adapter 分发 | 函数返回值 | 本地执行，无 LLM、无网络依赖 |
| **`cli`** | 子进程调用（`command` 模板） | stdout / 输出文件 | 本地已安装的外部扫描工具 |
| **`skill`** | CLI 不直接调用——由 Agent 层编排 | 用户/Agent 提供结果文件路径 | skill-ledger 以 Skill 形式运行；或手动指定其它 Skill 扫描结果 |
| **`api`** | HTTP POST 至 `endpoint` | 响应体 | 远端扫描服务 |

**`skill` 类型的关键约束**：skill-ledger CLI 不能直接调用 Skill（Skill 需要 Agent/LLM）。因此 `type: skill` 是**声明式**的：

- 声明"扫描器 X 是一个 Skill，其输出格式为 Y"
- `scan` 只自动调用已实现 adapter 的内置 `builtin` 扫描器，不调用 `skill` 类型扫描器
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
| **`findings-array`** | 恒等变换——输入已是 `[{rule, level, message, ...}]` | skill-vetter、code-scanner、static-scanner 及任何符合标准格式的扫描器 |
| **`sarif`** | 预留：读取 SARIF v2.1 JSON，映射 `results[].level` → `level`，`results[].ruleId` → `rule` | 工业标准静态分析工具 |
| **`field-mapping`** | 预留：用户定义 JSONPath 映射，从扫描器字段映射到 NormalizedFinding 字段 | 输出 JSON 但字段名不同的简单扫描器 |
| **`custom`** | 预留：用户提供 Python 可调用对象（入口点或模块路径） | 无法声明式映射的复杂/私有格式 |

**Level 映射（预留）**：未来的 `field-mapping` / `sarif` parser 可通过 `levelMap` 将扫描器原生的严重级别映射到 `deny | warn | pass`。当前已实现的 `findings-array` parser 要求输入中直接提供 `level` 字段：

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

#### 内置快速扫描器

当前版本默认注册并可自动调用两个内置扫描器：

- **`code-scanner`**：复用 agent-sec-core 的代码扫描组件，扫描 Skill 目录中的 Python / shell 类代码文件。
- **`static-scanner`**：基于 Cisco skill-scanner 静态规则设计的本地静态适配器，不调用 YARA、LLM、远端服务或完整上游包。
- **输出**：两者均输出 `findings-array` 格式（无需额外 parser）。
- **定位**：快速扫描捕获明显静态风险，不替代 Agent 驱动的深度语义审查。

未来如需新增其它内置扫描器，需要提供对应 adapter；仅编辑配置不足以让未知 `builtin` 名称自动运行。

#### Parser 查找逻辑

`scan` 与 `certify` 在生成 `ScanEntry` 前，都会根据 scanner 名称在 `scanners[]` → `parsers{}` 中查找对应 parser，执行归一化。未注册的 scanner 回退到 `findings-array`（向后兼容）。

#### 设计原则

1. **Ledger ≠ Scanner** — skill-ledger 追踪完整性并签名 manifest。扫描是输入而非核心职责。`scan` 是内置快速扫描编排入口，知道哪些 `builtin` scanner 可调用；`certify` 是外部 findings 导入入口，负责解析并签名入账。

2. **Parser 作为归一化层** — 通用合约是 `NormalizedFinding`，而非原始扫描器格式。这使异构扫描器可组合。

3. **`skill` 类型是声明式的** — CLI 不调用 Skill；仅声明其存在，使 `certify` 知道使用哪个 parser 处理其输出。Agent 层编排不在 CLI 职责范围内。

4. **优雅降级** — 若无 parser 匹配，回退到 `findings-array`。当前内置快速扫描器已使用该默认格式；未来新增其它输出格式时需实现对应 parser。

5. **独立发布周期** — `skill` 类型扫描器和符合 `findings-array` 的外部结果可以通过配置声明并由 `certify --findings` 消费；新的 `builtin` 自动调用能力需要对应 adapter 实现，`cli` / `api` adapter 仍是预留扩展点。

---

## 4. skill-ledger Skill（快速扫描 + 可选深度扫描）

### Skill 结构

```
skill-ledger/
  SKILL.md
  references/skill-vetter-protocol.md
```

### Phase 1：环境准备与状态查看

Agent 调用此 Skill 后，先按 SKILL.md 指令确认 CLI 可用、签名密钥存在，并解析目标 Skill 目录。状态查看模式只运行：

```bash
agent-sec-cli skill-ledger check <skill_dir>
# 或
agent-sec-cli skill-ledger check --all
```

### Phase 2：快速扫描认证

主动扫描和安装后认证默认执行快速扫描。快速扫描由 CLI 自动调用已注册且已实现 adapter 的内置 `builtin` 扫描器，当前使用：

```bash
agent-sec-cli skill-ledger scan <skill_dir>
# 或
agent-sec-cli skill-ledger scan --all
```

快速扫描完成后，Agent 再运行 `check` / `check --all` 读取最终状态并输出用户报告。报告中使用“快速扫描”称呼，不需要向用户展开内部扫描器名称。若需要限定扫描器，可使用 `--scanners code-scanner,static-scanner`；旧名称仅做兼容 alias。

### Phase 3：深度扫描认证（skill-vetter）

深度扫描仅在用户显式要求，或快速扫描后用户确认继续时执行。Agent 读取本 Skill 的 `references/skill-vetter-protocol.md`，按协议逐文件审查目标 Skill，并输出 `NormalizedFinding[]` JSON 数组到临时文件：

```text
/tmp/skill-vetter-findings-<skill_name>.json
```

每条 finding 必须使用 `rule`、`level`、`message`、`file`、`line`、`metadata` 等 `findings-array` parser 可识别的字段。随后调用：

```bash
agent-sec-cli skill-ledger certify <skill_dir> --findings /tmp/skill-vetter-findings-<skill_name>.json --scanner skill-vetter --delete-findings
```

`skill-vetter` 在注册表中仍是 `type: "skill"`：CLI 不会自动调用它，只负责解析其 findings 并签名写入 manifest。

---

## 5. Hook 默认策略

### 设计原则

hook 层（`skill-ledger check`）采用统一默认策略，不依赖用户自定义配置：

- `pass`：静默放行。
- `warn` / `error` / `unknown`：放行并告警，提示用户后续复查。
- `none` / `drifted` / `deny` / `tampered`：要求用户确认后继续。

fail-open 仅用于基础设施异常：CLI 不可用、执行失败、超时或输出不可解析时，hook 不阻断 Skill 加载，并通过宿主日志记录诊断信息。

### 各状态的行为

| 状态 | 行为 | 告警内容 |
|------|------|---------|
| `pass` | 静默放行 | 无 |
| `warn` | 放行 + 告警 | `⚠️ Skill '<name>' 存在低风险项，建议关注` |
| `error` | 放行 + 告警 | `⚠️ Skill '<name>' 状态检查返回错误，建议复查` |
| `unknown` | 放行 + 告警 | `⚠️ Skill '<name>' 返回未知状态，建议复查` |
| `drifted` | 用户确认 | `⚠️ Skill '<name>' 内容已变更，尚未重新扫描` |
| `none` | 用户确认 | `⚠️ Skill '<name>' 尚未经过安全扫描` |
| `deny` | 用户确认 | `🚨 Skill '<name>' 上次扫描存在高危项，请尽快处理` |
| `tampered` | 用户确认 | `🚨 Skill '<name>' 元数据签名校验失败，建议重新扫描建版` |

`none` / `drifted` / `deny` / `tampered` 均进入用户确认路径。`tampered` 触发条件较窄（内容未变但 manifest 被伪造），属于元数据可信度问题；仍需要用户确认是否继续加载，并建议重新执行扫描建版。

所有告警均通过宿主系统日志/消息通道输出，保证可追溯。

### 后续升级路径

当前策略为统一默认策略。后续可按需升级为可配置策略，例如对不同 Skill 来源设置不同确认门槛，或对 `drifted`/`none` 状态配置自动触发扫描建版。升级时仅需修改 hook handler 的返回值，不影响 CLI 和 Skill 侧逻辑。

### 向后兼容

若 `check` 遇到无签名的 `.skill-meta/`（升级前遗留数据），视为 `none` 而非 `tampered`。首次执行 `scan` 或 `certify` 后将自动补签。

---

## 6. 宿主集成

skill-ledger 需适配两个宿主系统，两者 Skill 模型和 Hook 机制存在本质差异：

| 维度 | OpenClaw | copilot-shell |
|------|---------|---------------|
| Skill 调用方式 | Agent 通过 read tool 读取 SKILL.md | Agent 调用 `Skill` tool，框架加载返回内容 |
| Hook 机制 | Plugin Hook（进程内 async handler） | Command Hook（fork 子进程，stdin/stdout JSON） |
| 告警输出 | `api.logger.warn`；需要确认时返回 `requireApproval` | `decision: "allow"` + `reason`；需要确认时返回 `decision: "ask"` |
| Skill 安装路径 | `~/.openclaw/skills/` | `~/.copilot-shell/skills/` |

两个实现共享相同的语义：拦截 Skill 加载 → 调用 `skill-ledger check` → `pass` 静默放行，`warn`/`error`/`unknown` 告警放行，`none`/`drifted`/`deny`/`tampered` 要求用户确认。

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

**Skill 目录定位（当前版本范围）**：copilot-shell hook 仅覆盖 project → user → system 三类 skill：
- project：`<cwd>/.copilot-shell/skills/<skill>/`
- user：`~/.copilot-shell/skills/<skill>/`
- system：`/usr/share/anolisa/skills/<skill>/`

当 PreToolUse 事件包含 `skill_context.file_path` 时，hook 优先使用该路径解决 `SKILL.md` 中 `name` 与目录名不一致的问题；但该路径仍必须落在上述 project/user/system 根目录内。若路径落在 custom、extension、remote 或其他目录，当前版本不执行 skill-ledger 检查，hook fail-open，并仅写入 debug 日志说明该 skill 不在当前 hook 支持范围内。

**custom / extension / remote Skills**：当前版本的 copilot-shell hook 不覆盖这些来源。未来若扩展覆盖范围，需要单独补充目录解析、信任边界和测试用例。
