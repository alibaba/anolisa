# Agent Sec Core

[English](README.md)

**面向 AI Agent 的 OS 级安全内核。** 提供系统加固、资产完整性校验与安全决策的完整防护链，作为所有业务 skill 之上的安全监督层运行，适用于 [ANOLISA](../../README_CN.md)、OpenClaw 等 AI Agent 运行平台。

## 背景

随着 AI Agent 逐步获得操作系统级别的执行能力（文件读写、网络访问、进程管理等），传统应用安全边界已不再适用。Agent Sec Core 从 **OS 层面** 为 Agent 构建纵深防御体系，确保 Agent 在受控、可审计、最小权限的环境中运行。

## 核心原则

1. **最小权限** — Agent 仅获得完成任务所需的最小系统权限。
2. **显式授权** — 敏感操作必须经过用户明确确认，禁止静默提权。
3. **零信任** — Skill 间互不信任，每次操作独立鉴权。
4. **纵深防御** — 系统加固 → 资产校验 → 安全决策，任一层失守不影响其他层。
5. **安全优先于执行** — 当安全与功能冲突时，安全优先；存疑时按高风险处理。

## 安全防护架构

```
┌─────────────────────────────────────────────┐
│              Agent Application              │
├──────────────────┬──────────────────────────┤
│ 安全检查工作流     │  沙箱策略                 │
│ (SKILL.md)       │  (agent-sec-sandbox,      │
│                  │   独立管理)               │
├──────────────────┴──────────────────────────┤
│  4. 安全决策流程（风险分级与处置）          │
├─────────────────────────────────────────────┤
│  Phase 3: 最终安全确认                       │
├─────────────────────────────────────────────┤
│  Phase 2: 关键资产保护 (GPG + SHA-256)       │
├─────────────────────────────────────────────┤
│  Phase 1: 系统安全加固 (loongshield)         │
├─────────────────────────────────────────────┤
│              Linux Kernel                   │
└─────────────────────────────────────────────┘
```

安全检查工作流（Phase 1-3 + 安全决策）定义在 `skill/SKILL.md` 中。沙箱策略由 `skill/references/agent-sec-sandbox.md` 独立管理。

## 安全检查工作流

每次 Agent 执行时，必须先按顺序完成以下安全检查（Phase 1-3），**全部通过后才允许进入安全决策流程**。完整可执行协议详见 `skill/SKILL.md`。

| 阶段 | 说明 | 入口 | 通过条件 |
|------|------|------|----------|
| **Phase 1** | 系统安全加固 — `loongshield seharden --scan --config agentos_baseline` | `skill/references/agent-sec-seharden.md` | 输出包含 `结果：合规` |
| **Phase 2** | 关键资产保护 — GPG 签名 + SHA-256 哈希校验所有 skill | `skill/references/agent-sec-skill-verify.md` | 输出包含 `VERIFICATION PASSED` |
| **Phase 3** | 最终安全确认 — 重新执行 Phase 1 scan + Phase 2 verify 作为复检 | `skill/SKILL.md` | 复检全部通过 |

任一 Phase 未通过，后续 Phase 全部取消，Agent 执行被阻断。

## 风险分级与处置

| 风险等级 | 典型场景 | 处置策略 |
|---------|---------|---------|
| **低** | 文件读取、信息查询、文本处理 | 允许，沙箱内执行 |
| **中** | 代码执行、包安装、调用外部 API | 沙箱隔离 + 用户确认 |
| **高** | 读取 `.env`/SSH 密钥、数据外发、修改系统配置 | 阻断，除非用户显式批准 |
| **危急** | Prompt injection、secret 外泄、禁用安全策略 | 立即阻断 + 审计日志 + 通知用户 |

**存疑时，按高风险处理。**

## 受保护资产

### 系统凭证

绝不允许 Agent 访问或外传：

- SSH 密钥（`/etc/ssh/`、`~/.ssh/`）
- GPG 私钥
- API tokens / OAuth credentials
- 数据库凭证
- `/etc/shadow`、`/etc/gshadow`
- 主机标识信息（IP、MAC、`hostname`）

### 系统关键文件

以下路径受写保护：

- `/etc/passwd`、`/etc/shadow`、`/etc/sudoers`
- `/etc/ssh/sshd_config`、`/etc/pam.d/`、`/etc/security/`
- `/etc/sysctl.conf`、`/etc/sysctl.d/`
- `/boot/`、`/usr/lib/systemd/`、`/etc/systemd/system/`

## 沙箱策略模板

`linux-sandbox` 提供 3 种内置策略模板：

| 模板 | 文件系统 | 网络 | 使用场景 |
|------|---------|------|---------|
| **read-only** | 全盘只读 | 禁止 | 只读操作：`ls`、`cat`、`grep`、`git status` 等 |
| **workspace-write** | cwd + /tmp 可写，其余只读 | 禁止 | 构建、编辑、脚本执行等需要写文件的操作 |
| **danger-full-access** | 无限制 | 允许 | ⚠ 保留模板，仅供特殊场景手动指定 |

命令分类直接映射沙箱模式：

| 分类 | 沙箱模式 | 说明 |
|------|---------|------|
| `destructive` | ❌ 拒绝执行 | 危险命令，直接拒绝 |
| `dangerous` | workspace-write | 高风险操作，不允许额外补权限 |
| `safe` | read-only | 只读操作，无需补权限 |
| `default` | workspace-write | 常规操作，可按需补网络/写路径 |

## 项目结构

```
agent-sec-core/
├── linux-sandbox/             # Rust 沙箱执行器（bubblewrap + seccomp）
│   ├── src/                   # Rust 源码（cli, policy, seccomp, proxy, …）
│   ├── tests/                 # Rust 集成测试 + Python e2e
│   └── docs/                  # dev-guide, user-guide
├── agent-sec-cli/             # 统一 CLI + 安全中间层（Python）
│   ├── src/agent_sec_cli/     # 主 Python 包
│   │   ├── cli.py             # CLI 入口点（Typer）
│   │   ├── asset_verify/      # Skill 签名 + 哈希校验
│   │   ├── code_scanner/      # 代码安全扫描引擎
│   │   ├── sandbox/           # 沙箱策略生成
│   │   ├── skill_ledger/      # Ed25519 完整性账本（check/certify/status）
│   │   ├── security_events/   # JSONL 事件日志
│   │   └── security_middleware/ # 中间层 + 后端实现
│   ├── dev-tools/             # 后端扩展开发指南
│   └── pyproject.toml         # 构建配置
├── skill/
│   ├── SKILL.md               # 可执行安全协议（检查工作流 + 安全决策）
│   └── references/
│       ├── agent-sec-seharden.md       # Phase 1 子 skill（loongshield 安全加固）
│       ├── agent-sec-sandbox.md        # 沙箱策略配置指南
│       └── agent-sec-skill-verify.md   # Phase 2 子 skill（资产校验）
├── tools/                     # sign-skill.sh — PGP 技能签名工具
├── tests/                     # 单元测试、集成测试、端到端测试
├── LICENSE
├── Makefile
├── agent-sec-core.spec        # RPM 打包 spec
├── README.md
└── README_CN.md
```

## 快速开始

### 前置条件

| 组件 | 要求 |
|------|------|
| **操作系统** | Alibaba Cloud Linux / Anolis / RHEL 系列 |
| **权限** | root 或 sudo |
| **loongshield** | >= 1.1.1（Phase 1 系统加固核心依赖） |
| **gpg / gnupg2** | >= 2.0（Phase 2 资产签名校验） |
| **Python3** | >= 3.6 |
| **Rust** | >= 1.91（用于构建 linux-sandbox） |

### 执行安全工作流

```bash
# ===== Phase 1: 系统安全加固 =====
# 基线扫描
sudo loongshield seharden --scan --config agentos_baseline

# 预演修复动作（可选）
sudo loongshield seharden --reinforce --dry-run --config agentos_baseline

# 执行自动加固
sudo loongshield seharden --reinforce --config agentos_baseline

# ===== Phase 2: 关键资产保护 =====
# 校验全部 skill 完整性
agent-sec-cli verify

# 校验单个 skill（可选）
agent-sec-cli verify --skill /path/to/skill_name

# ===== Phase 3: 最终安全确认 =====
# 复检确认合规
sudo loongshield seharden --scan --config agentos_baseline
agent-sec-cli verify
```

### 从源码构建沙箱

```bash
make build-sandbox
```

二进制文件输出到 `linux-sandbox/target/release/linux-sandbox`。

### RPM 安装

```bash
sudo yum install agent-sec-core
```

### 生成沙箱策略

对命令进行安全分类，生成 `linux-sandbox` 执行策略：

```bash
python3 agent-sec-cli/src/agent_sec_cli/sandbox/sandbox_policy.py --cwd "$PWD" "git status"
```

输出示例：
```json
{
  "decision": "sandbox",
  "classification": "safe",
  "sandbox_mode": "read-only",
  "sandbox_command": "linux-sandbox --sandbox-policy-cwd ... -- git status"
}
```

## 资产完整性校验

### 校验流程

1. 加载受信公钥（`agent-sec-cli/asset-verify/trusted-keys/*.asc`）
2. 验证 Skill 目录中 `.skill-meta/Manifest.json` 的 GPG 签名（`.skill-meta/.skill.sig`）
3. 校验 Manifest 中所有文件的 SHA-256 哈希

### 错误码

| 码 | 含义 |
|----|------|
| 0 | 通过 |
| 10 | 缺失 `.skill-meta/.skill.sig` |
| 11 | 缺失 `.skill-meta/Manifest.json` |
| 12 | 签名无效 |
| 13 | 哈希不匹配 |

### Skill 签名（自行部署快速开始）

通过源码部署时，skill 默认未签名。签名后 Phase 2 才能通过：

```bash
# 1. 一次性初始化：生成 GPG 密钥 + 导出公钥
tools/sign-skill.sh --init

# 2. 批量签名所有 skill
tools/sign-skill.sh --batch /usr/share/anolisa/skills --force

# 3. 验证
agent-sec-cli verify
```

完整指南（手动密钥管理、自定义 skill、CI/CD、问题排查）请参见 **[Skill 签名指南](tools/SIGNING_GUIDE_CN.md)**。

## Skill Ledger

基于 Ed25519 的 Skill 目录完整性账本。在 `.skill-meta/` 中记录文件哈希、版本链和扫描结果，通过 `agent-sec-cli skill-ledger` 子命令统一管理。

### 核心命令

| 命令 | 说明 |
|------|------|
| `init-keys` | 生成 Ed25519 签名密钥对 |
| `check <dir>` | 检测 Skill 文件是否漂移或被篡改 |
| `certify <dir>` | 运行扫描器、签名并封存清单 |
| `status` | 系统级健康概览（密钥、配置、聚合完整性） |
| `audit <dir>` | 查看版本历史与签名链 |
| `check --all` / `certify --all` | 对所有已注册 Skill 目录批量执行 |

### 快速示例

```bash
# 生成签名密钥（一次性）
agent-sec-cli skill-ledger init-keys

# 检查完整性（首次运行自动创建无签名基线）
agent-sec-cli skill-ledger check /path/to/skill

# 审查通过后认证
agent-sec-cli skill-ledger certify /path/to/skill

# 系统健康概览
agent-sec-cli skill-ledger status
```

设计文档：[`docs/design/SKILL_LEDGER_CN.md`](docs/design/SKILL_LEDGER_CN.md) · 用户指南：[`docs/guide/SKILL_LEDGER_USER_GUIDE_CN.md`](docs/guide/SKILL_LEDGER_USER_GUIDE_CN.md)

## 审计日志

所有安全事件以 JSONL 格式记录至 `/var/log/agent-sec/security-events.jsonl`（回退路径：`~/.agent-sec-core/security-events.jsonl`）：

```json
{"event_id": "uuid", "event_type": "harden", "category": "hardening", "timestamp": "ISO-8601", "trace_id": "uuid", "pid": 1234, "uid": 0, "details": {"request": {...}, "result": {...}}}
```

## 开发

```bash
# 构建沙箱
make build-sandbox

# 运行 Rust 测试
cd linux-sandbox && cargo test

# 运行端到端测试（需先安装沙箱）
python3 tests/e2e/linux-sandbox/e2e_test.py

# 格式化 Python 代码
make python-code-pretty
```

## 许可证

Apache License 2.0 — 详见 [LICENSE](../../LICENSE)。
