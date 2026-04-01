# 从源码构建 ANOLISA

[English](BUILDING.md)

本指南介绍如何准备开发环境、从源码构建各组件、运行测试以及构建 RPM 包。

提供两种构建路径：

1. 快速开始：运行一个脚本自动检查/安装依赖并构建选定组件。
2. 分组件构建：手动逐一构建各模块。

## 仓库结构

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

## 环境依赖

| 组件 | 所需工具 |
|------|----------|
| copilot-shell | Node.js >= 20、npm >= 10、make、g++ |
| os-skills | Python >= 3.12（仅可选脚本需要） |
| agent-sec-core | Rust == 1.93.0、Python >= 3.12、uv（仅 Linux） |
| agentsight *（可选）* | Rust >= 1.80、clang >= 14、libbpf 头文件、内核头文件（仅 Linux） |
| RPM 打包 | rpmbuild（仅 Linux） |

## 快速开始

使用统一构建脚本：

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa

# 安装依赖 + 构建 + 安装到系统（推荐）
./scripts/build-all.sh --install-deps --install

# 安装依赖 + 仅构建（不安装到系统）
./scripts/build-all.sh --install-deps

# 仅安装依赖
./scripts/build-all.sh --deps-only

# 仅构建指定组件
./scripts/build-all.sh --install-deps --component cosh --component sec-core

# 包含可选的 agentsight
./scripts/build-all.sh --install-deps --install --component cosh --component skills --component sec-core --component sight
```

### 脚本选项

| 参数 | 说明 |
|------|------|
| --install-deps | 构建前先安装依赖 |
| --deps-only | 仅安装依赖，不构建 |
| --install | 构建完成后将组件安装到系统路径 |
| --component <名称> | 构建指定组件（可重复使用）：cosh、skills、sec-core、sight。默认：cosh、skills、sec-core |
| --help | 显示帮助信息 |

### 注意事项

1. Node.js 和 Rust 建议通过上游安装器（nvm / rustup）安装，而非使用发行版软件包。
2. os-skills 大部分是静态资源，无需编译。
3. AgentSight 是**可选组件** — 它提供审计和可观测性能力，但不是核心功能所必需的。默认构建不包含它，使用 `--component sight` 显式包含。
4. AgentSight 的系统依赖（clang/llvm/libbpf/内核头文件）需通过发行版包管理器安装。

---

## 分组件构建

如果你希望手动设置各工具链并逐个构建组件，而非使用统一构建脚本，请按以下四个步骤操作。

### 步骤一：安装依赖

#### Node.js（用于 copilot-shell）

要求：Node.js >= 20、npm >= 10。

**Alinux 4（已验证）**

```bash
sudo dnf install -y nodejs npm make gcc-c++
```

**其他发行版：nvm**

```bash
# 如果 Node.js >= 20 已安装则跳过
if command -v node &>/dev/null && node -v | grep -qE '^v(2[0-9]|[3-9][0-9])'; then
  echo "Node.js $(node -v) 已安装，跳过"
else
  # 安装 nvm
  curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
  source "$HOME/.$(basename "$SHELL")rc"

  # 安装并激活 Node.js 20+
  nvm install 20
  nvm use 20
fi

# 验证
node -v   # 期望：v20.x.x 或更高
npm -v    # 期望：10.x.x 或更高
```
---

#### Rust（用于 agent-sec-core 和 agentsight）

要求：agent-sec-core 需要 Rust == 1.93.0；agentsight 需要 Rust >= 1.80。

**Alinux 4（已验证）**

```bash
sudo dnf install -y rust cargo gcc make
```

**Ubuntu 24.04（已验证）**

```bash
sudo apt install -y rustc-1.91 cargo-1.91 gcc make
sudo update-alternatives --install /usr/bin/cargo cargo /usr/bin/cargo-1.91 100
```

> 系统 `rust` 包的版本可能低于 1.93.0。如果 agent-sec-core 构建因版本不匹配而失败，请改用下方的 rustup。

**其他发行版：rustup**

```bash
# 如果 Rust 已安装则跳过
if command -v rustc &>/dev/null && command -v cargo &>/dev/null; then
  echo "Rust $(rustc --version) 已安装，跳过"
else
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
fi

# 验证
rustc --version   # 期望：rustc 1.80.0 或更高
cargo --version   # 期望：cargo 1.80.0 或更高
```

> 仓库为 agent-sec-core 固定了工具链版本（`rust-toolchain.toml`）。如果系统 Rust 版本不匹配，rustup 会在仓库内构建时自动下载正确版本。

---

#### Python 和 uv（用于 agent-sec-core 和 os-skills）

要求：Python >= 3.12。

**Alinux 4（已验证）**

```bash
pip3 install uv
uv python install 3.12
```

**Ubuntu 24.04（已验证）**

```bash
sudo apt install -y pipx
pipx ensurepath
source "$HOME/.$(basename "$SHELL")rc"
pipx install uv
```

**其他发行版：uv**

```bash
# 如果 uv 已安装则跳过
if command -v uv &>/dev/null; then
  echo "uv $(uv --version) 已安装，跳过"
else
  curl -LsSf https://astral.sh/uv/install.sh | sh
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

#### AgentSight 系统依赖（可选，需包管理器）

AgentSight 是**可选组件**，提供基于 eBPF 的审计和可观测性能力。它不是 ANOLISA 核心功能所必需的。如果你选择构建它，需要以下系统级依赖：

**dnf（Alinux / Anolis OS / Fedora / RHEL / CentOS / etc.）**

```bash
sudo dnf install -y clang llvm libbpf-devel elfutils-libelf-devel zlib-devel openssl-devel perl perl-IPC-Cmd
sudo dnf install -y kernel-devel-$(uname -r)
```

**apt（Debian / Ubuntu）**

```bash
sudo apt-get update -y
sudo apt-get install -y clang llvm libbpf-dev libelf-dev zlib1g-dev libssl-dev perl linux-headers-$(uname -r)
```

> 部分发行版没有单独的 perl-core 包，这是正常的。

**内核要求**

AgentSight 要求 Linux 内核 >= 5.10 且启用 BTF（`CONFIG_DEBUG_INFO_BTF=y`）。

---

#### 版本检查

```bash
node -v            # v20.x.x
npm -v             # 10.x.x
rustc --version    # rustc 1.80.0+
cargo --version    # cargo 1.80.0+
python3 --version  # Python 3.12.x
uv --version       # uv 0.x.x
clang --version    # clang version 14+
```

---

### 步骤二：构建组件

#### copilot-shell

```bash
cd src/copilot-shell
make install
make build
npm run bundle
```

产物：

- dist/cli.js

#### 运行

```bash
# 从构建目录直接运行
node dist/cli.js

# 或添加持久的 co 别名到你的 shell
make create-alias
source "$HOME/.$(basename "$SHELL")rc"
co
```

#### os-skills

无需编译。每个技能是一个目录，包含 `SKILL.md` 及可选的辅助文件（脚本、参考资料等）。部署时会将整个技能目录复制到目标路径。

#### 安装

Copilot Shell 从以下三个搜索路径发现技能：

| 范围 | 路径 |
|------|------|
| 项目级 | `.copilot/skills/` |
| 用户级 | `~/.copilot/skills/` |
| 系统级 | `/usr/share/anolisa/skills/` |

手动部署（用户级）：

```bash
# 构建脚本会自动复制技能：
./scripts/build-all.sh --component skills

# 或手动复制：
mkdir -p ~/.copilot/skills
find src/os-skills -name 'SKILL.md' -exec sh -c \
	'cp -rp "$(dirname "$1")" ~/.copilot/skills/' _ {} \;
```

#### 验证

```bash
# Copilot Shell 列出已发现的技能
co /skills
```

#### agent-sec-core（仅 Linux）

```bash
cd src/agent-sec-core
make build-sandbox
```

产物：

- linux-sandbox/target/release/linux-sandbox

#### 安装

```bash
sudo make install
```

#### agentsight（可选，仅 Linux）

> **注意：** AgentSight 是可选组件，提供基于 eBPF 的审计和可观测性能力，不是 ANOLISA 核心功能所必需的。

```bash
cd src/agentsight
make build
```

产物：

- target/release/agentsight

#### 安装

```bash
sudo make install
```

### 步骤三：运行测试（推荐）

#### 统一入口

```bash
./tests/run-all-tests.sh
./tests/run-all-tests.sh --filter shell
./tests/run-all-tests.sh --filter sec
./tests/run-all-tests.sh --filter sight
```

#### 分组件测试

```bash
# copilot-shell
cd src/copilot-shell && npm test

# agent-sec-core
cd src/agent-sec-core
pytest tests/integration-test/ tests/unit-test/

# agentsight
cd src/agentsight && cargo test
```

---

## 常见问题排查

### Node.js 版本不匹配

使用 nvm 重新激活期望版本：

```bash
source "$HOME/.$(basename "$SHELL")rc"
```

### Rust 工具链不匹配

```bash
rustup show
```

### AgentSight 缺少 libbpf / 头文件

按上方 AgentSight 依赖章节安装发行版软件包。

### AgentSight 运行时权限被拒绝

```bash
sudo ./target/release/agentsight --help
# 或
sudo setcap cap_bpf,cap_perfmon=ep ./target/release/agentsight
```
