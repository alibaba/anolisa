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
├─────────────────────────────────────────────┤
│         安全决策流程（风险分级与处置）          │
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

## 安全检查工作流

每次 Agent 执行时，必须先按顺序完成以下安全检查（Phase 1-3），全部通过后才允许进入安全决策流程：

| 阶段 | 说明 | 入口 |
|------|------|------|
| **Phase 1** | 系统安全加固 — 执行 `loongshield seharden --config agentos_baseline` 完成基线扫描和加固 | `skill/references/agent-sec-seharden.md` |
| **Phase 2** | 关键资产保护 — GPG 签名校验系统级 skill（Manifest + 文件哈希），确保 skill 完整性 | `skill/references/agent-sec-skill-verify.md` |
| **Phase 3** | 最终安全确认 — 汇总 Phase 1-2 状态，确认安全基线完整后方可进入安全决策流程 | `skill/SKILL.md` |

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
├── skill/
│   ├── SKILL.md               # 完整使用指南（安全工作流 + 安全决策流程）
│   ├── scripts/
│   │   ├── sandbox/
│   │   │   ├── sandbox_policy.py     # 沙箱策略生成器
│   │   │   ├── classify_command.py   # 命令分类器
│   │   │   └── rules.py              # 分类规则定义
│   │   └── asset-verify/
│   │       ├── verifier.py         # Skill 签名 + 哈希校验
│   │       ├── errors.py           # 错误码定义
│   │       ├── config.conf         # skills 目录配置
│   │       └── trusted-keys/       # 受信公钥目录
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
python3 skill/scripts/asset-verify/verifier.py

# 校验单个 skill（可选）
python3 skill/scripts/asset-verify/verifier.py --skill /path/to/skill_name

# ===== Phase 3: 最终安全确认 =====
# 复检确认合规
sudo loongshield seharden --scan --config agentos_baseline
python3 skill/scripts/asset-verify/verifier.py
```

### 从源码构建

如果你克隆仓库时没有使用 `--recursive`，请先执行一次：

```bash
git submodule update --init --recursive src/agent-sec-core/third_party/loongshield
```

然后执行：

```bash
make build-all
```

这会始终构建 `linux-sandbox`，并尝试构建内置在 `third_party/loongshield` 的 `loongshield` 子模块。
如果宿主机不满足 `loongshield` 自身的 `make env-check`，则会跳过 `loongshield` 构建，但 `agent-sec-core` 仍然可以构建成功。

产物：
- `linux-sandbox/target/release/linux-sandbox`
- `third_party/loongshield/build/src/daemon/loongshield`，仅在宿主机支持构建 loongshield 时生成

### RPM 安装

```bash
sudo yum install agent-sec-core
```

### 生成沙箱策略

对命令进行安全分类，生成 `linux-sandbox` 执行策略：

```bash
python3 skill/scripts/sandbox/sandbox_policy.py --cwd "$PWD" "git status"
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

1. 加载受信公钥（`skill/scripts/asset-verify/trusted-keys/*.asc`）
2. 验证 Skill 目录中 `Manifest.json` 的 GPG 签名（`.skill.sig`）
3. 校验 Manifest 中所有文件的 SHA-256 哈希

### 错误码

| 码 | 含义 |
|----|------|
| 0 | 通过 |
| 10 | 缺失 `.skill.sig` |
| 11 | 缺失 `Manifest.json` |
| 12 | 签名无效 |
| 13 | 哈希不匹配 |

### 签名技能

```bash
sign-skill.sh <技能目录>
```

## 开发

```bash
# 构建 sandbox，并尝试构建内置 loongshield 依赖
make build-all

# 仅尝试构建内置 loongshield 依赖
make loongshield

# 运行 Rust 测试
cd linux-sandbox && cargo test

# 运行端到端测试（需先安装沙箱）
python3 tests/e2e/linux-sandbox/e2e_test.py

# 安装完整本地运行时
sudo make install

# 格式化 Python 代码
make python-code-pretty
```

## 许可证

Apache License 2.0 — 详见 [LICENSE](../../LICENSE)。
