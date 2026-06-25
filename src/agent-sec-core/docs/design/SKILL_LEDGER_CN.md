# Skill 安全技术方案（skill-ledger）

## 背景与目标

### 问题

AI Agent 通过加载 Skill（结构化指令 + 辅助脚本）扩展能力。Skill 来源多样（官方内置、社区分发、用户自建），可指示 Agent 执行 shell 命令、读写文件等高权限操作。当前缺乏对 Skill 内容完整性和安全性的系统化验证机制——恶意或被篡改的 Skill 可静默获取 Agent 的全部工具权限。

### 设计目标

1. **防篡改**：通过密码学签名的版本链（SignedManifest）保护 Skill 元数据，使篡改可被检测
2. **安全扫描集成**：提供可扩展的扫描器框架，支持 Agent 驱动（skill-vetter）和 CLI 自动调用两种模式
3. **运行态激活**：推荐与 SkillFS 联合使用，由 SkillFS 捕获 Skill 变更并通知 Skill Ledger daemon 刷新 activation metadata
4. **可用性优先**：SkillFS 只消费 activation target；宿主 hook 只消费统一 exposure summary 的提示信息，CLI 异常、超时或输出不可解析时保持 fail-open

### 非目标

- 不替代操作系统级沙箱或进程隔离
- 不实现运行时行为监控（仅静态内容检查 + 签名验证）
- 不实现按 skill/来源区分的细粒度 activation 策略；当前全局 `activationPolicy` 只保留 `pass_warn_only` 行为

---

## 1. 整体架构

```
┌───────────────────────────────────────────────────────┐
│              宿主系统 + SkillFS                         │
│                                                       │
│  ┌──────────────┐       ┌──────────────────────────┐ │
│  │  SkillFS     │       │  Agent 工作区              │ │
│  │  捕获变更     │       │                          │ │
│  │      │        │       │  ┌──────────────────┐   │ │
│  │      ▼        │       │  │  skill-ledger    │   │ │
│  │ daemon notify │       │  │  (Skill)         │   │ │
│  │      │        │       │  │                  │   │ │
│  │      ▼        │       │  │  Phase 1: 状态   │   │ │
│  │ activation    │       │  │  Phase 2: 快扫   │   │ │
│  │ refresh       │       │  │  Phase 3: 深扫   │   │ │
│  │      │        │       │  │  scan/certify 签名 │   │ │
│  │      ▼        │       │  │                  │   │ │
│  │ activation.json/xattr │  └──────────────────┘   │ │
│  └───────────────┘       └──────────────────────────┘ │
│          │                          │                  │
│          └──── .skill-meta/ ────────┘                  │
│                                                       │
│  ~/.local/share/agent-sec/skill-ledger/                     │
│    key.enc (私钥)     ← scan/certify 签名                   │
│    check 只读状态；scan/certify 创建签名版本与 snapshot       │
│    key.pub (公钥)     ← check 验签                     │
└───────────────────────────────────────────────────────┘
```

**组件职责**：

- **skill-ledger CLI**：核心基础设施。提供 `init`（初始化密钥并可为已覆盖 Skill 建立快速扫描 baseline）、`scan`（运行内置快速扫描器并签名入账）、`check`（只读检查 JSON + 验签 + 比哈希 + 输出状态，可供宿主 hook/capability 调用）、`certify`（导入外部 findings 并签名）等子命令。`scan` / `certify` 写入的 manifest 经 Ed25519 数字签名保护，防止篡改；`check` 在无 manifest 时返回 `none`，不创建版本或 snapshot。确定性逻辑不依赖 LLM，不可被 prompt injection 绕过。
- **Scanner Registry**：可扩展扫描框架。通过配置注册扫描器（`builtin`/`cli`/`skill`/`api` 四种调用类型）和结果解析器（将异构扫描输出归一化为统一 `NormalizedFinding` 格式）。本版本默认注册 `skill-vetter`（`type: "skill"`，由 Agent 深度扫描后通过 `certify --findings` 消费）、`code-scanner` 和 `static-scanner`（均为 `type: "builtin"`，可由 `scan` 自动调用）。当前仅实现 `findings-array` parser；`cli`/`api` adapter 及其它 parser 类型为预留扩展点。旧名称 `skill-code-scanner`、`cisco-static-scanner` 仅作为兼容 alias 读取，不再作为公开名称展示或写入新 manifest。
- **skill-ledger Skill**：一个 Skill，三个阶段。Phase 1 做环境准备与状态查看；Phase 2 默认执行快速扫描认证（`scan` 调用内置 `code-scanner` 与 `static-scanner`）；Phase 3 在用户显式要求或确认后执行 Agent 驱动深度扫描（`skill-vetter`），再用 `certify --findings ... --delete-findings` 写入版本链。
- **SkillFS + daemon activation**：推荐运行态入口。SkillFS 捕获 Skill 文件变化后调用 daemon 的 `skill_ledger.skillfs_notify_change` 接口，daemon 根据签名 manifest 和 activation policy 刷新 `.skill-meta/activation.json`，并尽力同步写入 xattr。
- **Hook / capability 兼容层**：宿主可挂载 `skill-ledger show` 作为兼容入口。默认发行配置继续挂载/注册，`policy = "ask"`；hook 只读取统一 exposure summary 中的 `message`，当用户已经通过 `decide` 做出决策时保持静默。

---

## 2. 数据模型与安全架构

### 目录结构

```
<skill_dir>/
├── ...                        # Skill 文件（不修改）
└── .skill-meta/
    ├── latest.json            # 最新 manifest（certify 后含数字签名）
    ├── versions/
    │   ├── v000001.json       # 首版 manifest（由 scan/certify/init baseline 签名创建）
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

  "policy": "warning",             // legacy 兼容字段；新逻辑不读取
  "userDecision": null,             // 可选用户决策：allow | always_allow | block | rollback

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

**userDecision**：用户对某个具体签名版本的运行态决策，字段为：
- `action`：`allow` / `always_allow` / `block` / `rollback`
- `targetVersionId`：仅 `rollback` 必填，记录回退来源版本
- `reason`：可选的人类可读原因

`allow` 只允许当前版本；`always_allow` 允许当前版本并继承到后续新版本，直到用户改为其它决策或清除；`block` 只作用于当前最新版本，不继承到未来版本；`rollback` 会把目标 snapshot 恢复到 skill 根目录，扫描生成新版本，并在新版本记录回退决策。`userDecision = null` 表示没有用户决策，此时回到全局 activation 行为。旧 `policy` 字段仅为兼容保留，新逻辑不读取。

`userDecision` 参与 `manifestHash` 与签名计算，这是为了防止用户决策被离线篡改。带 `userDecision` 字段的 manifest 属于本版本 schema；混合部署时，旧版 `agent-sec-cli` 可能无法按新字段重算相同 manifest hash，从而把该 manifest 判为 tampered。生产环境应将写入与读取 Skill Ledger manifest 的组件一起升级。

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
  "activationPolicy": "pass_warn_only", // 当前唯一运行态策略；旧 pass_only/latest_scanned 会被归一化
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

**合并策略**：默认目录默认启用，由 `enableDefaultSkillDirs` 控制；`managedSkillDirs` 存放 skill-ledger 动态管理或用户额外配置的目录，不再兼容旧的 `skillDirs` 字段。解析时默认目录在前，`managedSkillDirs` 在后，自动去重。`scanners` 按 `name` 合并，用户配置可覆盖同名扫描器；`activationPolicy` 是全局运行态策略，当前只执行 `pass_warn_only` 行为，历史配置值 `pass_only` / `latest_scanned` 会兼容读取并归一化；`signingBackend` 当前会被读取到配置摘要中，但不会改变实际签名后端。

**自动记忆**：用户对某个 skill 执行 `scan` 或 `certify` 时，若该 skill 目录不在当前有效目录中，会自动追加到 `managedSkillDirs`。`check` 是只读状态检查，不会写配置、manifest 或 snapshot。若父目录下有 ≥2 个包含 `SKILL.md` 的兄弟 skill，则追加父目录 glob（`parent/*`）而非单个路径。追加后自动压缩（compact）：若某 glob 已覆盖某个单目录条目，则移除冗余的单目录条目。

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
| `check` | 低层完整性与扫描状态检查（只读 JSON） | 已实现 |
| `certify` | 导入外部 findings 并签名写入 manifest | 已实现 |
| 内部 resolver | 写入运行态 activation（daemon 内部调用，不提供 CLI） | 已实现 |
| `show` | 展示统一 exposure summary、findings 与 root/active 一致性 | 已实现 |
| `export` | 导出指定 snapshot、manifest 和 findings 供用户审查 | 已实现 |
| `decide` | 写入或清除用户决策，并刷新 activation | 已实现 |
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

**`skill-ledger check <skill_dir>`** — 低层完整性与扫描状态检查

判定流程（按优先级）：

1. **无 manifest** → 返回 `none`；不创建版本、manifest 或 snapshot
2. **fileHashes 不匹配** → 返回 `drifted`（附 added/removed/modified 详情）
3. **签名验证失败** → 返回 `tampered`
4. **签名有效** → 按 `scanStatus` 返回 `deny` / `warn` / `none` / `pass`

输出为单行 JSON。`check` 始终只读，不需要私钥，也不会签名；后续已签名 manifest 的验签仅需公钥。宿主 hook 的用户提示入口是 `show`，不是直接解析 `check.status`。

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
| **一：对齐** | 确保 manifest 与磁盘文件一致 | 无 manifest、drifted 或 tampered 时按当前文件创建新版本；`check` 只读，不创建版本 |
| **二：导入** | 获取扫描结果 | 读取外部 findings 文件，输出经 parser 归一化为 `NormalizedFinding[]` |
| **三：签名** | 更新 manifest 并签名 | 合并 scan 条目 → 聚合 `scanStatus`（取最严重级别）→ 重算 `manifestHash` → Ed25519 签名 → 原子写入 |

**内部 activation resolver** — 写入运行态 activation

Skill Ledger 不提供面向用户或 SkillFS 的 `resolve` CLI；activation refresh 是 daemon 内部职责。resolver 调用统一 `build_exposure_summary()`，根据签名版本链、用户决策和 `pass_warn_only` activation 行为选择可运行 target，并原子写入 `.skill-meta/activation.json`，同时尽力同步写入 skill 目录 xattr `user.agent_sec.skill_ledger.activation`：

```json
{
  "schemaVersion": 1,
  "target": ".skill-meta/versions/v000002.snapshot"
}
```

`activation.json` 仍只有 `{schemaVersion,target}`。SkillFS 不理解 `scanStatus`、`userDecision`、findings 或版本链，只按 target 暴露 snapshot；`target:null` 表示隐藏该 skill：

```json
{
  "schemaVersion": 1,
  "target": null
}
```

resolver 状态规则：

| 场景 | target | message |
|------|--------|---------|
| `userDecision.action in {"allow","always_allow","rollback"}` | 指向该决策允许的真实 snapshot | `null` |
| `userDecision.action == "block"` | `null` | `null` |
| 无用户决策且 latest 为 `pass` / `warn` | 指向 latest 可信 snapshot | `null` |
| 无用户决策且 latest 为 `deny` / `none` / `drifted` / `tampered`，存在上一个可信 `pass` / `warn` | 指向 fallback snapshot | 说明 latest 风险状态和当前 active 版本 |
| 无用户决策且 latest 风险状态没有可信 fallback | 指向 `.skill-meta/versions/__pending_decision__.snapshot` | 说明当前暴露安全占位并提示 `show` / `export` / `decide` |

pending decision stub 不是 ledger 版本，不对应 `v000001.json` manifest，也不能作为 rollback 默认目标。它只包含安全 `SKILL.md` 占位内容，使 Agent 仍能通过普通 SkillFS discovery 发现该 skill，但真实风险内容不暴露。用户通过 `decide allow` / `decide rollback` / `decide block` 做出决策后，resolver 会切换到真实 snapshot 或 `target:null`，并清理 stale pending stub。

`pass_warn_only` 是当前唯一运行态行为：只允许签名、manifest hash、snapshot 完整且 `scanStatus in {"pass","warn"}` 的 snapshot 由全局策略直接暴露。历史配置值 `pass_only` / `latest_scanned` 仅为兼容读取，会被归一化为 `pass_warn_only`，不再产生独立分支。resolver 始终只激活 snapshot，不激活 source/current 工作区。daemon 在收到 SkillFS 变更通知、扫描完成、用户决策或重启 reconcile 时调用该 resolver。

**`skill-ledger decide <skill_dir>`** — 写入或清除用户决策

用户对风险 skill 的决策入口：

```bash
agent-sec-cli skill-ledger decide <skill_dir> --action allow [--reason TEXT]
agent-sec-cli skill-ledger decide <skill_dir> --action always_allow [--reason TEXT]
agent-sec-cli skill-ledger decide <skill_dir> --action block [--reason TEXT]
agent-sec-cli skill-ledger decide <skill_dir> --action rollback [--version v000001] [--reason TEXT]
agent-sec-cli skill-ledger decide <skill_dir> --clear
```

`--clear` 将 latest manifest 的 `userDecision` 置空，恢复全局 activation 行为。`rollback` 未指定 `--version` 时，默认选择当前用户决策或 `pass_warn_only` 策略下的真实 active version；若当前只有 pending stub 或 hidden，没有真实 active version，则报错。

**`skill-ledger show <skill_dir>`** — 展示统一暴露摘要和诊断信息

`show` 复用 `build_exposure_summary()`，输出 `latestStatus`、`latestVersionId`、`activeVersionId`、`target`、`userDecision`、`reasonCode`、`message`，并附带 findings、root 与 active 是否一致等人工诊断信息。hook 只消费其中的 `message`。

**`skill-ledger export <skill_dir> --version latest|active|v000001 --output <path>`** — 导出签名 snapshot 供审查

`export` 会把目标版本的 `snapshot/`、`manifest.json` 和 `findings.json` 导出到指定目录。pending stub 不是真实 active version，因此在 pending 状态下 `--version active` 会报错；用户应使用 `--version latest` 审查被隐藏的风险版本。

**`skill-ledger status [--verbose]`** — 查询整体安全状况（系统级概览）

返回 skill-ledger 系统的整体健康状态，包含三个区块：
- `keys`：签名密钥基础设施状态（是否已初始化、指纹、是否加密、归档密钥数量）
- `config`：配置摘要（默认目录、managedSkillDirs 模式数、已注册扫描器列表）
- `skills`：聚合健康度（已发现 Skill 数量、各状态计数、整体 `health` 标签：`healthy` / `unscanned` / `attention` / `critical` / `empty`）

使用 `--verbose` 时额外输出 `results` 数组，包含每个已注册 Skill 的详细检查结果。与 `check` 的定位区分：`check` 是单个 Skill 的完整性门禁（供 hook/plugin 调用，退出码语义化），`status` 是系统级态势感知（始终退出码 0，纯信息输出）。

**`skill-ledger list-scanners`** — 查看已注册扫描器

列出内置默认及 `~/.config/agent-sec/skill-ledger/config.json` 中注册的扫描器，包括公开名称、调用类型、结果解析器、启用状态和 `autoInvocable`。默认只展示 canonical 名称：`code-scanner`、`static-scanner`、`skill-vetter`；旧名称只作为兼容 alias 读取。用于发现 `scan --scanners` 和 `certify --scanner` 可用的扫描器名称。

**`skill-ledger audit <skill_dir>`** — 深度校验版本链完整性

遍历 `versions/` 逐版本验证 manifestHash、签名、`previousManifestSignature` 链接完整性。可选 `--verify-snapshots` 校验快照文件哈希，并拒绝 snapshot 中的 symlink、特殊文件和 `.skill-meta` / `.git` 元数据路径。输出结构化校验结果。

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

## 5. 可选 Hook / capability 兼容策略

### 设计原则

推荐部署模式是 SkillFS 捕获变更并触发 daemon activation refresh。宿主 hook/capability 作为兼容入口保留并默认挂载，默认 `policy = "ask"`：当 `skill-ledger show` 的统一 exposure summary 返回 `message != null` 时，由宿主请求用户确认；当 `userDecision != null` 或 summary 静默时，不再提示。

hook/capability 不直接读取 `check.status`，也不自行实现扫描状态分支。所有实现共享流程：

```text
拦截 Skill 加载或读取 -> 调用 agent-sec-cli skill-ledger show <skill_dir> -> 读取 summary.message -> 按宿主 policy 呈现
```

Policy 语义：

| Policy | 行为 |
|--------|------|
| `ask` | 默认值。`message == null` 静默放行；`message != null` 时请求用户确认或使用宿主 approval UI。 |
| `warn` | `message == null` 静默放行；`message != null` 时展示 warning 并放行。 |
| `debug` | `message != null` 时只写 debug 诊断并放行。 |
| `block` | `message != null` 时直接阻断，并把 message 作为原因或告警信息。 |

CLI 不可用、执行失败、超时或输出不可解析属于基础设施异常，hook/capability 保持 fail-open。`block_statuses` / `blockStatuses` 是旧配置字段，新逻辑不再按状态列表判断；旧 `enable_block` / `enableBlock` 仅在未配置 `policy` 时作为兼容映射。

### message 触发规则

`message` 来自统一 exposure summary：

| Summary 状态 | Hook 行为 |
|--------------|-----------|
| `userDecision != null` | 必须静默，不弹框、不告警。 |
| `latestStatus == pass` 且 active 为 latest | 静默。 |
| `latestStatus == warn` 且 active 为 latest | 静默；`warn` 与 `pass` 在默认暴露逻辑中对齐。 |
| 无用户决策，latest 为 `deny` / `none` / `drifted` / `tampered`，且 active 回退到旧 `pass` / `warn` | 展示 message，说明 latest 风险状态和当前 active 版本。 |
| 无用户决策，latest 风险状态且没有真实 fallback | 展示 message，说明当前暴露安全 pending review stub，并提示用户执行 `show` / `export` / `decide`。 |

### 向后兼容

若 `check` 遇到无签名的 `.skill-meta/`（升级前遗留数据），视为 `none` 而非 `tampered`。首次执行 `scan` 或 `certify` 后将自动补签。宿主 hook 仍可在内部调用 `show` 间接获得该状态，但不再直接以 `check` 输出作为用户提示依据。

---

## 6. 宿主集成

推荐宿主集成由 SkillFS 驱动：SkillFS 负责捕获 Skill 目录变更，通知 Skill Ledger daemon 扫描并刷新 `.skill-meta/activation.json`/xattr。宿主 hook/capability 作为兼容路径保留并默认使用 `ask` policy，各宿主的 Skill 模型和 Hook 机制存在差异：

| 维度 | OpenClaw | copilot-shell | Hermes |
|------|---------|---------------|--------|
| Skill 调用方式 | Agent 通过 read tool 读取 SKILL.md | Agent 调用 `Skill` tool，框架加载返回内容 | Agent 调用 `skill_view` 读取 Skill |
| Hook 机制 | Plugin Hook（进程内 async handler） | Command Hook（fork 子进程，stdin/stdout JSON） | Plugin Hook（`pre_tool_call` + `transform_llm_output`） |
| `ask` policy | 返回 `requireApproval` | 返回 `decision: "ask"` | 通过 `transform_llm_output` 注入提示 |
| `warn` policy | `api.logger.warn` 后放行 | `decision: "allow"` + `reason` | 缓存本轮 warning，并追加到最终回复开头 |
| `debug` policy | `api.logger.debug` 后放行 | stderr debug 后放行 | logger debug 后放行 |
| `block` policy | 返回 `block` / `blockReason` | 返回 `decision: "block"` | 返回 `{"action": "block"}` |
| Skill 安装路径 | `~/.openclaw/skills/` | project / user / system 三类路径 | 当前 hook 覆盖 `~/.hermes/skills/**` |
| 默认启用状态 | `enabled=true, policy="ask"` | 默认 manifest 注册 hook，`SKILL_LEDGER_HOOK_POLICY=ask` | `enabled=true, policy="ask"` |

### 6.1 OpenClaw（Plugin Hook）

以 OpenClaw Plugin 形式分发，默认注册 `skill-ledger` capability，`capabilities.skill-ledger.policy` 默认为 `ask`。`before_tool_call` handler 过滤 read tool 对 `*/SKILL.md` 的访问，解析 `skill_dir` 后调用 `agent-sec-cli skill-ledger show`。`enabled=false` 时完全不注册；`policy=warn` 时通过 `api.logger.warn` 输出告警并放行；`policy=debug` 只写 debug；`policy=block` 在 summary message 非空时返回 `block`。旧 `enableBlock` 仅在未配置 `policy` 时作为兼容映射，旧 `blockStatuses` 不再参与运行态判断。

### 6.2 copilot-shell（Command Hook）

独立 Python 脚本 `cosh-extension/hooks/skill_ledger_hook.py`，专为 stdin/stdout 协议设计，不依赖 `agent_sec_cli` 包。默认 Cosh manifest 挂载该 hook，默认 `SKILL_LEDGER_HOOK_POLICY=ask`。该环境变量属于可信宿主或部署环境配置，不应由 Skill、项目脚本或不可信 shell 启动逻辑设置；若需要防止本地 shell profile 被篡改后降级策略，后续应迁移到可信宿主配置源：

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

### 6.3 Hermes（Plugin Hook）

以 Hermes Plugin 形式分发，默认 `capabilities.skill-ledger.enabled=true` 且 `policy="ask"`。`pre_tool_call` handler 过滤 `skill_view`，仅根据 `name` / `skill` / `skill_name` 在 Hermes 默认本地目录 `~/.hermes/skills` 下解析 Skill 目录后调用 `agent-sec-cli skill-ledger show`。`file_path` / `path` 在 Hermes 中表示 Skill 内 supporting file，不作为 Skill 身份来源。若无法解析、匹配到多个候选、命中 `~/.hermes/config.yaml` 的 `skills.external_dirs` 或 plugin-provided skills 等当前未覆盖来源，hook 采用 fail-open。`policy=ask` / `warn` 时，summary message 会记录为本轮 warning，并由 `transform_llm_output` 追加到最终回复开头；`policy=debug` 只写 debug；`policy=block` 在 message 非空时直接阻断本次 `skill_view`。`max_warnings_per_turn = 0` 只影响 `ask` / `warn` 的用户可见 warning 注入。旧 `enable_block` 仅在未配置 `policy` 时作为兼容映射，旧 `block_statuses` 不再参与运行态判断。

Skill Ledger 全局 `activationPolicy` 属于 SkillFS/daemon activation，宿主 hook 的 `policy` 只控制 hook/capability 的用户可见行为和日志等级。
