# Skill 签名指南

[English](SIGNING_GUIDE.md)

通过源码构建和部署 ANOLISA 时，部署的 skill 默认是**未签名**的。agent-sec-core 安全工作流的 Phase 2 要求有效的 GPG 签名——在每个 skill 目录包含已签名的 `.skill-meta/Manifest.json` 之前，完整性校验将无法通过。

`sign-skill.sh`（位于本目录）提供了所需的全部功能：前置依赖检查、GPG 密钥生成、批量签名、公钥导出。

## 前置依赖

| 工具 | RHEL / Anolis / Alinux | Debian / Ubuntu | 用途 |
|------|----------------------|-----------------|------|
| **gpg**（gnupg2） | `sudo yum install -y gnupg2` | `sudo apt-get install -y gnupg` | GPG 签名与验证 |
| **jq** | `sudo yum install -y jq` | `sudo apt-get install -y jq` | JSON Manifest 生成 |
| **sha256sum** | `coreutils`（通常已预装） | `coreutils`（通常已预装） | 文件哈希计算 |

检查前置依赖：

```bash
tools/sign-skill.sh --check
```

## 快速开始

三条命令即可完成全部流程。步骤 1 每台机器只需执行一次；步骤 2 在 skill 文件变更后需重新执行。

```bash
# 1. 一次性初始化 — 生成 GPG 密钥并导出公钥到 trusted-keys 目录
tools/sign-skill.sh --init

# 2. 批量签名所有已部署的 skill（默认：~/.copilot-shell/skills/）
tools/sign-skill.sh --batch --force

# 3. 验证
agent-sec-cli verify
```

`--init` 会自动生成专用签名密钥（`ANOLISA Local Deploy Key`），并将公钥导出到
`~/.copilot-shell/skills/agent-sec-core/scripts/asset-verify/trusted-keys/`。
可通过 `--trusted-keys-dir <DIR>` 覆盖导出路径。

## 手动逐步操作

如果你希望完全控制 GPG 密钥管理，而不使用 `--init`：

### 1. 生成 GPG 密钥

```bash
gpg --batch --gen-key <<EOF
Key-Type: RSA
Key-Length: 4096
Name-Real: My Signing Key
Name-Email: me@example.com
Expire-Date: 2y
%no-protection
%commit
EOF
```

确认密钥已创建：

```bash
gpg --list-secret-keys me@example.com
```

### 2. 导出公钥

校验器从 `~/.copilot-shell/skills/agent-sec-core/scripts/asset-verify/trusted-keys/` 加载受信公钥，
`--init` 会自动导出到此目录。手动重新导出：

```bash
tools/sign-skill.sh --export-key
```

或导出到自定义目录：

```bash
tools/sign-skill.sh --export-key /custom/path/to/trusted-keys/
```

或完全手动导出：

```bash
gpg --armor --export me@example.com \
    > ~/.copilot-shell/skills/agent-sec-core/scripts/asset-verify/trusted-keys/me-example-com.asc
```

### 3. 签名 Skill

签名单个 skill：

```bash
tools/sign-skill.sh /usr/share/anolisa/skills/my-skill --force
```

批量签名目录下所有 skill：

```bash
# 使用默认目录（~/.copilot-shell/skills/）
tools/sign-skill.sh --batch --force

# 或指定自定义目录
tools/sign-skill.sh --batch /usr/share/anolisa/skills --force
```

签名后每个 skill 目录将包含：

| 文件 | 说明 |
|------|------|
| `.skill-meta/Manifest.json` | skill 内所有文件的 SHA-256 哈希 |
| `.skill-meta/.skill.sig` | `Manifest.json` 的 GPG 分离签名 |

### 4. 配置校验器

使用 `--batch` 时，脚本会自动将 skill 目录注册到 `config.conf` 中。如果手动配置，请确保 skill 目录已配置在已部署的 `config.conf` 中（如 `~/.copilot-shell/skills/agent-sec-core/scripts/asset-verify/config.conf`）：

```ini
skills_dir = [
    /usr/share/anolisa/skills
]
```

### 5. 验证

```bash
# 验证所有已配置目录
agent-sec-cli verify

# 验证单个 skill
agent-sec-cli verify --skill /usr/share/anolisa/skills/my-skill
```

成功时的预期输出：

```
[OK] my-skill

==================================================
PASSED: 1
FAILED: 0
==================================================
VERIFICATION PASSED
```

## 签名自定义 Skill

如果你创建了自定义 skill 并与内置 skill 一起部署：

1. 将 skill 目录（包含 `SKILL.md`）放到 skill 根目录下，例如 `/usr/share/anolisa/skills/my-custom-skill/`。
2. 签名：
   ```bash
   tools/sign-skill.sh /usr/share/anolisa/skills/my-custom-skill --force
   ```
3. 确保 skill 根目录已配置在 `config.conf` 中（见上方第 4 步）。
4. 验证：
   ```bash
   agent-sec-cli verify --skill /usr/share/anolisa/skills/my-custom-skill
   ```

## CI/CD 签名

在 CI/CD 流水线中（GPG 密钥环未预配置），通过 `GPG_PRIVATE_KEY` 环境变量传入私钥，脚本会在签名前自动导入：

```bash
export GPG_PRIVATE_KEY="$(cat my-private-key.asc)"
tools/sign-skill.sh --batch /path/to/skills --force
```

如果密钥有密码保护：

```bash
export GPG_PRIVATE_KEY="$(cat my-private-key.asc)"
export GPG_PASSPHRASE="my-passphrase"
tools/sign-skill.sh --batch /path/to/skills --force
```

## Skill 更新后重新签名

每当 skill 文件被修改，已有的 `.skill-meta/Manifest.json` 哈希值将失效。使用 `--force` 重新签名：

```bash
tools/sign-skill.sh --batch --force
```

然后验证：

```bash
agent-sec-cli verify
```

## 校验错误码

| 码 | 含义 | 常见原因 |
|----|------|---------|
| 0 | 通过 | — |
| 10 | 缺失 `.skill-meta/.skill.sig` | skill 从未签名 |
| 11 | 缺失 `.skill-meta/Manifest.json` | skill 从未签名 |
| 12 | 签名无效 | 签名密钥不在 `trusted-keys/` 中 |
| 13 | 哈希不匹配 | 签名后 skill 文件被修改 |

## sign-skill.sh 命令参考

| 模式 | 命令 | 说明 |
|------|------|------|
| **初始化** | `--init [--trusted-keys-dir DIR]` | 生成 GPG 密钥 + 导出公钥 |
| **检查** | `--check` | 检查前置依赖（gpg、jq、sha256sum） |
| **单个签名** | `<skill_dir> [--force]` | 签名单个 skill 目录 |
| **批量签名** | `--batch [parent_dir] [--force]` | 签名目录下所有子目录（默认：`~/.copilot-shell/skills/`）。自动将目录注册到 `config.conf`。 |
| **导出公钥** | `--export-key [DIR]` | 导出公钥（默认：`~/.copilot-shell/skills/agent-sec-core/scripts/asset-verify/trusted-keys/`） |

常用选项：

| 选项 | 说明 |
|------|------|
| `--force` | 覆盖已有的 `.skill-meta/Manifest.json` 和 `.skill-meta/.skill.sig` |
| `--skill-name NAME` | 覆盖 Manifest 中的 skill 名称（默认：目录名） |
| `--trusted-keys-dir DIR` | 覆盖公钥导出目录（配合 `--init` 使用） |
