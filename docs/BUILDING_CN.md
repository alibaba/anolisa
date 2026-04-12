# 从源码构建 ANOLISA

[English](BUILDING.md)

本指南介绍如何准备开发环境、从源码构建各组件并运行测试。

提供两种构建路径：

1. 快速开始：运行一个脚本自动检查/安装依赖并构建选定组件。

2. 分组件构建：手动逐一构建各模块。

## 1. 仓库结构

```text
anolisa/
├── src/
│   ├── copilot-shell/       # AI 终端助手（Node.js / TypeScript）
│   ├── os-skills/           # 运维技能库（Markdown + 可选脚本）
│   ├── agent-sec-core/      # Agent 安全沙箱（Rust + Python）
│   └── agentsight/          # eBPF 可观测/审计引擎（Rust，可选）
├── scripts/
│   ├── build-all.sh         # 统一构建入口
│   └── rpm-build.sh         # 统一 RPM 构建脚本
├── tests/
│   └── run-all-tests.sh     # 统一测试入口
├── Makefile
└── docs/
```

## 2. 环境依赖

| 组件 | 所需工具 |
|------|----------|
| copilot-shell | Node.js >= 20、npm >= 10、make、g++ |
| os-skills | Python >= 3.12（仅可选脚本需要） |
| agent-sec-core | Rust >= 1.91.0、Python >= 3.12、uv（仅 Linux） |
| agentsight *（可选）* | Rust >= 1.80、clang >= 14、libbpf 头文件、内核头文件（仅 Linux） |

## 3. 快速开始

统一构建脚本可自动完成依赖安装、构建和系统安装。

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa
```

克隆完成后，运行构建脚本。默认会安装依赖、构建并安装到系统：

> **提示：** 以下每个方式都是独立的命令，根据自己的需求选择一个执行即可。如果使用了统一构建脚本，可以跳过下方的[分组件构建](#4-分组件构建)部分。

```bash
# 默认：安装依赖 + 构建 + 安装到系统（推荐大多数用户使用）
./scripts/build-all.sh

# 仅构建，不安装到系统
./scripts/build-all.sh --no-install

# 跳过依赖安装（依赖已就绪时使用）
./scripts/build-all.sh --ignore-deps

# 仅安装依赖（适用于 CI 或手动构建场景）
./scripts/build-all.sh --deps-only

# 仅构建并安装指定组件
./scripts/build-all.sh --component cosh --component sec-core

# 包含可选的 agentsight
./scripts/build-all.sh --component cosh --component skills --component sec-core --component sight
```

### 3.1 脚本选项

| 参数 | 说明 |
|------|------|
| --no-install | 跳过将组件安装到系统路径 |
| --ignore-deps | 跳过依赖安装 |
| --deps-only | 仅安装依赖，不构建 |
| --component <名称> | 构建指定组件（可重复使用）：cosh、skills、sec-core、sight。默认：cosh、skills、sec-core |
| --help | 显示帮助信息 |

### 3.2 注意事项

1. 构建脚本会优先使用系统软件包，当系统版本不满足要求时自动回退到上游安装器（nvm / rustup）。
2. os-skills 大部分是静态资源，无需编译。
3. AgentSight 是可选组件，提供审计和可观测性能力，但不是核心功能所必需的。默认构建不包含它，使用 `--component sight` 显式包含。
4. AgentSight 的系统依赖（clang/llvm/libbpf/内核头文件）需通过发行版包管理器安装。
5. **沙箱策略 Hooks 不会自动启用。** 安装完成后，需在 Copilot Shell 内执行一次 `/hooks install` 来激活内置的 sandbox-guard hooks。详见 [3.3 安装后：启用沙箱 Hooks](#33-安装后启用沙箱-hooks)。

### 3.3 安装后：启用沙箱 Hooks

构建脚本完成安装后，沙箱策略 hooks 不会自动激活。为防止未经授权的文件系统访问或危险命令执行，请在安装完成后执行以下操作：

```bash
cosh
```

进入 Copilot Shell 会话后，执行：

```
/hooks install
```

该命令会：
- 将内置的 `sandbox-guard.py` 和 `sandbox-failure-handler.py` 脚本复制到 `~/.copilot-shell/hooks/`
- 将其注册为 `PreToolUse` 和 `PostToolUseFailure` hooks，写入用户配置（`~/.copilot-shell/settings.json`）
- 立即在当前会话中激活这些 hooks

只需执行一次，配置会持久保存，后续会话自动生效。

> **说明：** 沙箱 hooks 依赖 `agent-sec-core`（linux-sandbox）正确构建并安装到 `/usr/local/bin/linux-sandbox`。当 `sandbox-guard.py` 检测到危险命令时，会将其包裹为 `linux-sandbox ... bash -c '...'` 的形式执行。若 `agent-sec-core` 未构建安装，hooks 可以注册成功，但危险命令的沙箱隔离执行将会失败。请确保在依赖沙箱防护前已完成 `agent-sec-core` 的构建（默认 `build-all.sh` 已包含该步骤）。

---

## 4. 分组件构建

> **如果已使用上方的统一构建脚本，可以跳过本节。** 脚本会自动完成依赖安装、构建和系统安装的所有步骤。

如果你希望手动设置各工具链并逐个构建组件，请按以下步骤操作。

### 4.1 安装依赖

#### 4.1.1 Node.js（用于 copilot-shell）

要求：Node.js >= 20、npm >= 10。

- **Alinux 4（已验证）**

```bash
sudo dnf install -y nodejs npm make gcc-c++
```

- **其他发行版：nvm**

```bash
# 如果 Node.js >= 20 已安装则跳过
if command -v node &>/dev/null && node -v | grep -qE '^v(2[0-9]|[3-9][0-9])'; then
  echo "Node.js $(node -v) 已安装，跳过"
else
  # 从 Gitee 镜像安装 nvm
  curl -fsSL --connect-timeout 15 --max-time 60 https://gitee.com/mirrors/nvm/raw/v0.40.3/install.sh | bash
  source "$HOME/.$(basename "$SHELL")rc"

  # 配置 npmmirror 加速 Node.js 下载
  export NVM_NODEJS_ORG_MIRROR=https://npmmirror.com/mirrors/node/
  nvm install 20
  nvm use 20
fi

# 验证
node -v   # 期望：v20.x.x 或更高
npm -v    # 期望：10.x.x 或更高
```

---

#### 4.1.2 Rust（用于 agent-sec-core 和 agentsight）

要求：agent-sec-core 需要 Rust >= 1.91.0；agentsight 需要 Rust >= 1.80。

- **Alinux 4（已验证）** — 系统 `rust` 包版本低于 1.91.0，无法直接使用，请用下方 rustup 安装。
仅需通过 dnf 安装构建工具：

```bash
sudo dnf install -y gcc make
```

- **Ubuntu 24.04（已验证）**

```bash
sudo apt install -y rustc-1.91 cargo-1.91 gcc make
sudo update-alternatives --install /usr/bin/cargo cargo /usr/bin/cargo-1.91 100
```

> 部分发行版的系统 `rust` 包版本可能低于 1.91.0。如果构建因版本不匹配而失败，请改用下方的 rustup。

- **其他发行版 / Alinux 4：rustup（推荐）**

```bash
# 如果 Rust 已安装则跳过
if command -v rustc &>/dev/null && command -v cargo &>/dev/null; then
  echo "Rust $(rustc --version) 已安装，跳过"
else
  # 通过 rsproxy.cn 镜像安装 Rust
  curl --proto '=https' --tlsv1.2 -sSf --connect-timeout 15 --max-time 120 https://rsproxy.cn/rustup-init.sh | sh -s -- -y
  source "$HOME/.cargo/env"
fi

# 验证
rustc --version   # 期望：rustc 1.91.0 或更高
cargo --version   # 期望：cargo 1.91.0 或更高
```

> 仓库为 agent-sec-core 固定了工具链版本（`rust-toolchain.toml`）。如果系统 Rust 版本不匹配，rustup 会在仓库内构建时自动下载正确版本。

**配置 Rustup 分发镜像（国内用户推荐）**

如果构建时触发了固定工具链的自动下载（通过 `rust-toolchain.toml`）并且超时，请设置 Rustup 分发镜像：

```bash
export RUSTUP_DIST_SERVER="https://rsproxy.cn"
export RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
```

将以上内容添加到 shell 配置文件（`~/.bashrc` 或 `~/.zshrc`）中以使其永久生效。构建脚本（`build-all.sh`）会自动配置此项。

**配置 crates.io 镜像（国内用户推荐）**

如果 `cargo build` 拉取依赖较慢，可配置阿里云 crates.io 镜像。
构建脚本（`build-all.sh`）会自动配置，不限于 rustup 安装路径。
手动设置方法：在 `~/.cargo/config.toml` 中添加：

```toml
[source.crates-io]
replace-with = 'aliyun'
[source.aliyun]
registry = "sparse+https://mirrors.aliyun.com/crates.io-index/"
```

---

#### 4.1.3 Python 和 uv（用于 agent-sec-core 和 os-skills）

要求：Python >= 3.12。

- **Alinux 4（已验证）**

```bash
pip3 install uv
uv python install 3.12
```

- **Ubuntu 24.04（已验证）**

```bash
sudo apt install -y pipx
pipx ensurepath
source "$HOME/.$(basename "$SHELL")rc"
pipx install uv
```

- **其他发行版：uv**

```bash
# 如果 uv 已安装则跳过
if command -v uv &>/dev/null; then
  echo "uv $(uv --version) 已安装，跳过"
else
  # 安装 uv
  curl -LsSf --connect-timeout 15 --max-time 60 https://astral.sh/uv/install.sh | sh
  source "$HOME/.$(basename "$SHELL")rc"
fi

# 通过 uv 安装 Python 3.12（已存在则跳过）
uv python install 3.12
```

```bash
# 验证
uv --version          # 期望：uv 0.x.x 或更高
uv python find 3.12   # 期望：输出 python3.12 可执行文件路径
```

---

#### 4.1.4 AgentSight 系统依赖（可选，需包管理器）

AgentSight 是可选组件，提供基于 eBPF 的审计和可观测性能力，不是 ANOLISA 核心功能所必需的。如果你选择构建它，需要以下系统级依赖：

- **dnf（Alinux / Anolis OS / Fedora / RHEL / CentOS 等）**

```bash
sudo dnf install -y clang llvm libbpf-devel elfutils-libelf-devel zlib-devel openssl-devel perl perl-IPC-Cmd
sudo dnf install -y kernel-devel-$(uname -r)
```

- **apt（Debian / Ubuntu）**

```bash
sudo apt-get update -y
sudo apt-get install -y clang llvm libbpf-dev libelf-dev zlib1g-dev libssl-dev perl linux-headers-$(uname -r)
```

> 部分发行版没有单独的 perl-core 包，这是正常的。

- **内核要求**

AgentSight 要求 Linux 内核 >= 5.10 且启用 BTF（`CONFIG_DEBUG_INFO_BTF=y`）。

---

#### 4.1.5 版本检查

```bash
node -v            # v20.x.x
npm -v             # 10.x.x
rustc --version    # rustc 1.91.0+
cargo --version    # cargo 1.91.0+
python3 --version  # Python 3.12.x
uv --version       # uv 0.x.x
clang --version    # clang version 14+
```

---

### 4.2 构建组件

#### 4.2.1 copilot-shell

```bash
cd src/copilot-shell
make deps
make build
```

> **注意：** `make deps` 执行 `npm install`，会自动初始化 husky pre-commit 钩子。每次提交时，钩子会对暂存文件执行 Prettier 格式化和 ESLint 检查。CI 环境请使用 `make deps-ci`，该命令会跳过钩子安装。

产物：`dist/cli.js`

```bash
# 直接运行
node dist/cli.js

# 或安装到系统 PATH（创建 cosh/co/copilot 命令）
sudo make install
cosh
```

#### 4.2.2 os-skills

**安装**

技能搜索路径（Copilot Shell 按以下优先级发现技能）：

| 范围 | 路径 |
|------|------|
| 项目级 | `.copilot-shell/skills/` |
| 用户级 | `~/.copilot-shell/skills/` |
| 系统级 | `/usr/share/anolisa/skills/` |

安装方式：

- **使用构建脚本自动部署**

```bash
./scripts/build-all.sh --component skills
```

- **手动部署（用户级）**

```bash
mkdir -p ~/.copilot-shell/skills
find src/os-skills -name 'SKILL.md' -exec sh -c \
	'cp -rp "$(dirname "$1")" ~/.copilot-shell/skills/' _ {} \;
```

**验证**

```bash
co /skills
```

#### 4.2.3 agent-sec-core（仅 Linux）

```bash
cd src/agent-sec-core
make build-sandbox
```

产物：`linux-sandbox/target/release/linux-sandbox`

**安装**

```bash
sudo make install-sandbox
```

#### 4.2.4 agentsight（可选，仅 Linux）

> 注意：AgentSight 是可选组件，提供基于 eBPF 的审计和可观测性能力，不是 ANOLISA 核心功能所必需的。

```bash
cd src/agentsight
make build
```

产物：`target/release/agentsight`

**安装**

```bash
sudo make install
```

### 4.3 运行测试（推荐）

#### 4.3.1 统一入口

```bash
./tests/run-all-tests.sh
./tests/run-all-tests.sh --filter shell
./tests/run-all-tests.sh --filter sec
./tests/run-all-tests.sh --filter sight
```

#### 4.3.2 分组件测试

```bash
# copilot-shell
cd src/copilot-shell && make test

# agent-sec-core
cd src/agent-sec-core
pytest tests/integration-test/ tests/unit-test/

# agentsight
cd src/agentsight && cargo test
```

---

## 5. 常见问题排查

### 5.1 Node.js 版本不匹配

使用 nvm 重新激活期望版本：

```bash
source "$HOME/.$(basename "$SHELL")rc"
```

### 5.2 Rust 工具链不匹配

```bash
rustup show
```

### 5.3 AgentSight 缺少 libbpf / 头文件

按 4.1.4 节安装对应发行版的系统软件包。

### 5.4 AgentSight 运行时权限被拒绝

```bash
sudo ./target/release/agentsight --help
# 或授予最小权限
sudo setcap cap_bpf,cap_perfmon=ep ./target/release/agentsight
```
