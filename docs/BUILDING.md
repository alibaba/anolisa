# Building ANOLISA from Source

[中文版](BUILDING_CN.md)

This guide describes how to prepare the development environment, build each component from source, and run tests.

Two paths are provided:

1. Quick Start: run one script to check/install dependencies and build selected components.
2. Component-by-Component: build each module manually.

## 1. Repository Layout

```text
anolisa/
├── src/
│   ├── copilot-shell/       # AI terminal assistant (Node.js / TypeScript)
│   ├── os-skills/           # Ops skills (Markdown + optional scripts)
│   ├── agent-sec-core/      # Agent security sandbox (Rust + Python)
│   └── agentsight/          # eBPF observability/audit agent (Rust, optional)
├── scripts/
│   ├── build-all.sh         # Unified build entry
│   └── rpm-build.sh         # Unified RPM build script
├── tests/
│   └── run-all-tests.sh     # Unified test entry
├── Makefile
└── docs/
```

## 2. Environment Dependencies

| Component | Required Tools |
|-----------|----------------|
| copilot-shell | Node.js >= 20, npm >= 10, make, g++ |
| os-skills | Python >= 3.12 (only for optional scripts) |
| agent-sec-core | Rust >= 1.91.0, Python >= 3.12, uv (Linux only) |
| agentsight *(optional)* | Rust >= 1.80, clang >= 14, libbpf headers, kernel headers (Linux only) |

## 3. Quick Start

The unified build script handles dependency installation, building, and system installation automatically.

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa
```

Then run the build script. By default it installs dependencies, builds, and installs to the system:

> **Tip:** Each option below is a standalone command — just pick the one that fits your use case. If you use the unified script, you can skip the [Component-by-Component Build](#4-component-by-component-build) section below entirely.

```bash
# Default: install deps + build + install to system (recommended for most users)
./scripts/build-all.sh

# Build only, skip system install
./scripts/build-all.sh --no-install

# Skip dependency installation (deps already present)
./scripts/build-all.sh --ignore-deps

# Install dependencies only (useful for CI or manual builds)
./scripts/build-all.sh --deps-only

# Build and install selected components only
./scripts/build-all.sh --component cosh --component sec-core

# Include optional agentsight
./scripts/build-all.sh --component cosh --component skills --component sec-core --component sight
```

### 3.1 Script Options

| Flag | Description |
|------|-------------|
| --no-install | Skip installing built components to system paths |
| --ignore-deps | Skip dependency installation |
| --deps-only | Install dependencies only, do not build |
| --component <name> | Build selected component(s), repeatable: cosh, skills, sec-core, sight. Default: cosh, skills, sec-core |
| --help | Show help |

### 3.2 Important Notes

1. The build script tries system packages first and falls back to upstream installers (nvm / rustup) when system versions don't meet requirements.
2. os-skills are mostly static assets and do not require compilation.
3. AgentSight is an optional component that provides audit and observability capabilities but is not required for core functionality. It is excluded from default builds; use `--component sight` to include it explicitly.
4. AgentSight system dependencies (clang/llvm/libbpf/kernel headers) should be installed through your distro package manager.

---

## 4. Component-by-Component Build

> **You can skip this section** if you already used the unified build script above. The script handles all dependency installation, building, and system installation automatically.

If you prefer to set up each toolchain and build each component manually, follow the steps below.

### 4.1 Install Dependencies

#### 4.1.1 Node.js (for copilot-shell)

Required: Node.js >= 20, npm >= 10.

- **Alinux 4 (verified)**

```bash
sudo dnf install -y nodejs npm make gcc-c++
```

- **Other distros: nvm**

```bash
# Skip if Node.js >= 20 is already installed
if command -v node &>/dev/null && node -v | grep -qE '^v(2[0-9]|[3-9][0-9])'; then
  echo "Node.js $(node -v) already installed, skipping"
else
  # Install nvm (fallback to Gitee mirror if GitHub is unreachable)
  curl -fsSL --connect-timeout 15 https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash \
    || curl -fsSL https://gitee.com/mirrors/nvm/raw/v0.40.3/install.sh | bash
  source "$HOME/.$(basename "$SHELL")rc"

  # Install and activate Node.js 20+
  # Use npmmirror.com if default download is slow
  # export NVM_NODEJS_ORG_MIRROR=https://npmmirror.com/mirrors/node/
  nvm install 20
  nvm use 20
fi

# Verify
node -v   # expected: v20.x.x or higher
npm -v    # expected: 10.x.x or higher
```

---

#### 4.1.2 Rust (for agent-sec-core and agentsight)

Required: agent-sec-core needs Rust >= 1.91.0; agentsight needs Rust >= 1.80.

- **Alinux 4 (verified)** — the system `rust` package is below 1.91.0; use rustup instead (see below).
Only install the build tools from dnf:

```bash
sudo dnf install -y gcc make
```

- **Ubuntu 24.04 (verified)**

```bash
sudo apt install -y rustc-1.91 cargo-1.91 gcc make
sudo update-alternatives --install /usr/bin/cargo cargo /usr/bin/cargo-1.91 100
```

> The system `rust` package on some distros may be older than 1.91.0. If the build fails due to version mismatch, use rustup below.

- **Other distros / Alinux 4: rustup (recommended)**

```bash
# Skip if Rust is already installed
if command -v rustc &>/dev/null && command -v cargo &>/dev/null; then
  echo "Rust $(rustc --version) already installed, skipping"
else
  # Install Rust (fallback: Aliyun internal → Aliyun public → rsproxy.cn)
  curl --proto '=https' --tlsv1.2 -sSf --connect-timeout 15 https://sh.rustup.rs | sh -s -- -y \
    || curl -sSf --connect-timeout 5 http://mirrors.cloud.aliyuncs.com/repo/rust/rustup-init.sh | sh -s -- -y \
    || curl --proto '=https' --tlsv1.2 -sSf --connect-timeout 15 https://mirrors.aliyun.com/repo/rust/rustup-init.sh | sh -s -- -y \
    || curl --proto '=https' --tlsv1.2 -sSf https://rsproxy.cn/rustup-init.sh | sh -s -- -y
  source "$HOME/.cargo/env"
fi

# Verify
rustc --version   # expected: rustc 1.91.0 or higher
cargo --version   # expected: cargo 1.91.0 or higher
```

> The repository uses a pinned toolchain (`rust-toolchain.toml`) for agent-sec-core. If the system Rust version does not match, rustup will automatically download the correct version when building inside the repo.

**Configure crates.io mirror (recommended for China users)**

If `cargo build` is slow fetching dependencies, configure an Aliyun crates.io mirror.
The build script (`build-all.sh`) configures this automatically regardless of how Rust is installed.
For manual setup, add to `~/.cargo/config.toml`:

```toml
[source.crates-io]
replace-with = 'aliyun'
[source.aliyun]
registry = "sparse+https://mirrors.aliyun.com/crates.io-index/"
```

> On Aliyun ECS (VPC), replace `mirrors.aliyun.com` with `mirrors.cloud.aliyuncs.com` for faster internal access. See [Aliyun Rustup Mirror](https://developer.aliyun.com/mirror/rustup) for details.

---

#### 4.1.3 Python and uv (for agent-sec-core and os-skills)

Required: Python >= 3.12.

- **Alinux 4 (verified)**

```bash
pip3 install uv
uv python install 3.12
```

- **Ubuntu 24.04 (verified)**

```bash
sudo apt install -y pipx
pipx ensurepath
source "$HOME/.$(basename "$SHELL")rc"
pipx install uv
```

- **Other distros: uv**

```bash
# Skip if uv is already installed
if command -v uv &>/dev/null; then
  echo "uv $(uv --version) already installed, skipping"
else
  # Install uv (fallback to GitHub mirror if astral.sh is unreachable)
  curl -LsSf --connect-timeout 15 https://astral.sh/uv/install.sh | sh \
    || curl -LsSf https://github.com/astral-sh/uv/releases/latest/download/uv-installer.sh | sh
  source "$HOME/.$(basename "$SHELL")rc"
fi

# Install Python 3.12 via uv (skips if already present)
uv python install 3.12
```

```bash
# Verify
uv --version          # expected: uv 0.x.x or higher
uv python find 3.12   # expected: path to python3.12 binary
```

---

#### 4.1.4 AgentSight System Dependencies (Optional, Package Manager Required)

AgentSight is an optional component that provides eBPF-based audit and observability capabilities. It is not required for core ANOLISA functionality. If you choose to build it, the following system-level dependencies are needed:

- **dnf (Alinux / Anolis OS / Fedora / RHEL / CentOS / etc.)**

```bash
sudo dnf install -y clang llvm libbpf-devel elfutils-libelf-devel zlib-devel openssl-devel perl perl-IPC-Cmd
sudo dnf install -y kernel-devel-$(uname -r)
```

- **apt (Debian / Ubuntu)**

```bash
sudo apt-get update -y
sudo apt-get install -y clang llvm libbpf-dev libelf-dev zlib1g-dev libssl-dev perl linux-headers-$(uname -r)
```

> Some distributions do not provide a separate perl-core package. That is expected.

- **Kernel Requirement**

AgentSight requires Linux kernel >= 5.10 and BTF enabled (`CONFIG_DEBUG_INFO_BTF=y`).

---

#### 4.1.5 Version Check

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

### 4.2 Build Components

#### 4.2.1 copilot-shell

```bash
cd src/copilot-shell
make deps
make build
```

> **Note:** `make deps` runs `npm install`, which automatically sets up husky pre-commit hooks. These hooks run Prettier and ESLint on staged files before each commit. In CI environments, use `make deps-ci` instead, which skips hook installation.

Artifact: `dist/cli.js`

```bash
# Run directly
node dist/cli.js

# Or install to system PATH (creates cosh/co/copilot commands)
sudo make install
cosh
```

#### 4.2.2 os-skills

**Install**

Skill search paths (Copilot Shell discovers skills in the following priority order):

| Scope | Path |
|-------|------|
| Project | `.copilot/skills/` |
| User | `~/.copilot/skills/` |
| System | `/usr/share/anolisa/skills/` |

Install options:

- **Using the build script (automatic)**

```bash
./scripts/build-all.sh --component skills
```

- **Manual deployment (user-level)**

```bash
mkdir -p ~/.copilot/skills
find src/os-skills -name 'SKILL.md' -exec sh -c \
	'cp -rp "$(dirname "$1")" ~/.copilot/skills/' _ {} \;
```

**Verify**

```bash
co /skills
```

#### 4.2.3 agent-sec-core (Linux only)

```bash
cd src/agent-sec-core
make build-sandbox
```

Artifact: `linux-sandbox/target/release/linux-sandbox`

**Install**

```bash
sudo make install
```

#### 4.2.4 agentsight (Optional, Linux only)

> Note: AgentSight is an optional component. It provides eBPF-based audit and observability capabilities but is not required for core ANOLISA functionality.

```bash
cd src/agentsight
make build
```

Artifact: `target/release/agentsight`

**Install**

```bash
sudo make install
```

### 4.3 Run Tests (Recommended)

#### 4.3.1 Unified Entry

```bash
./tests/run-all-tests.sh
./tests/run-all-tests.sh --filter shell
./tests/run-all-tests.sh --filter sec
./tests/run-all-tests.sh --filter sight
```

#### 4.3.2 Per Component

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

## 5. Troubleshooting

### 5.1 Node.js version mismatch

Use nvm and re-activate the expected version:

```bash
source "$HOME/.$(basename "$SHELL")rc"
```

### 5.2 Rust toolchain mismatch

```bash
rustup show
```

### 5.3 AgentSight missing libbpf / headers

Install distro packages from section 4.1.4 above.

### 5.4 AgentSight runtime permission denied

```bash
sudo ./target/release/agentsight --help
# or grant minimum capabilities
sudo setcap cap_bpf,cap_perfmon=ep ./target/release/agentsight
```
