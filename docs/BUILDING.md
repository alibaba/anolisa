# Building ANOLISA from Source

[中文版](BUILDING_CN.md)

This guide describes how to prepare the development environment, build each component from source, run tests, and build RPM packages.

Two paths are provided:

1. Quick Start: run one script to check/install dependencies and build selected components.
2. Component-by-Component: build each module manually.

---

## 1. Repository Layout

```text
anolisa/
├── src/
│   ├── copilot-shell/       # AI terminal assistant (Node.js / TypeScript)
│   ├── os-skills/           # Ops skills (Markdown + optional scripts)
│   ├── agent-sec-core/      # Agent security sandbox (Rust + Python)
│   └── agentsight/          # eBPF observability/audit agent (Rust, optional)
├── scripts/
│   ├── build-all.sh         # Unified build entry (you will provide this script)
│   └── rpm-build.sh         # Unified RPM build script
├── tests/
│   └── run-all-tests.sh     # Unified test entry
├── Makefile
└── docs/
```

---

## 2. Environment Dependencies

| Component | Required Tools |
|-----------|----------------|
| copilot-shell | Node.js >= 20, npm >= 10, make, g++ |
| os-skills | Python >= 3.12 (only for optional scripts) |
| agent-sec-core | Rust == 1.93.0, Python >= 3.12, uv (Linux only) |
| agentsight *(optional)* | Rust >= 1.80, clang >= 14, libbpf headers, kernel headers (Linux only) |
| RPM packaging | rpmbuild (Linux only) |

---

## 3. Quick Start

First, clone the repository:

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa
```

Then choose one of the following operations based on your needs:

a. Install dependencies + build + install to system (recommended, all-in-one)

```bash
./scripts/build-all.sh --install-deps --install
```

b. Install dependencies + build only (without system install)

```bash
./scripts/build-all.sh --install-deps
```

c. Install dependencies only, skip build for now

```bash
./scripts/build-all.sh --deps-only
```

d. Install dependencies and build selected components (without agentsight)

```bash
./scripts/build-all.sh --install-deps --component cosh --component sec-core
```

e. Install dependencies and build all components (including optional agentsight)

```bash
./scripts/build-all.sh --install-deps --install --component cosh --component skills --component sec-core --component sight
```

### 3.1 Script Options

| Flag | Description |
|------|-------------|
| --install-deps | Install dependencies before build |
| --deps-only | Install dependencies only, skip build |
| --install | Install built components to system paths after building |
| --component \<name\> | Build selected component(s), repeatable: cosh, skills, sec-core, sight. Default: cosh, skills, sec-core |
| --help | Show help |

### 3.2 Important Notes

1. Node.js and Rust should be installed from upstream installers (nvm / rustup), not pinned to distro packages.
2. os-skills are mostly static assets and do not require compilation.
3. AgentSight is an optional component that provides audit and observability capabilities but is not required for core functionality. It is excluded from default builds; use `--component sight` to include it explicitly.
4. AgentSight system dependencies (clang/llvm/libbpf/kernel headers) should be installed through your distro package manager.

---

## 4. Component-by-Component Build

If you prefer to set up each toolchain and build each component manually instead of using the unified script, follow the steps below.

### 4.1 Install Dependencies

#### 4.1.1 Node.js (for copilot-shell)

Required: Node.js >= 20, npm >= 10.

a. Alinux 4 (verified)

```bash
sudo dnf install -y nodejs npm make gcc-c++
```

b. Other distros: nvm

```bash
# Skip if Node.js >= 20 is already installed
if command -v node &>/dev/null && node -v | grep -qE '^v(2[0-9]|[3-9][0-9])'; then
  echo "Node.js $(node -v) already installed, skipping"
else
  # Install nvm
  curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh | bash
  source "$HOME/.$(basename "$SHELL")rc"

  # Install and activate Node.js 20+
  nvm install 20
  nvm use 20
fi

# Verify
node -v   # expected: v20.x.x or higher
npm -v    # expected: 10.x.x or higher
```

---

#### 4.1.2 Rust (for agent-sec-core and agentsight)

Required: agent-sec-core needs Rust == 1.93.0; agentsight needs Rust >= 1.80.

a. Alinux 4 (verified)

```bash
sudo dnf install -y rust cargo gcc make
```

b. Ubuntu 24.04 (verified)

```bash
sudo apt install -y rustc-1.91 cargo-1.91 gcc make
sudo update-alternatives --install /usr/bin/cargo cargo /usr/bin/cargo-1.91 100
```

> The system `rust` package may be older than 1.93.0. If agent-sec-core build fails due to version mismatch, use rustup below instead.

c. Other distros: rustup

```bash
# Skip if Rust is already installed
if command -v rustc &>/dev/null && command -v cargo &>/dev/null; then
  echo "Rust $(rustc --version) already installed, skipping"
else
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
fi

# Verify
rustc --version   # expected: rustc 1.80.0 or higher
cargo --version   # expected: cargo 1.80.0 or higher
```

> The repository uses a pinned toolchain (`rust-toolchain.toml`) for agent-sec-core. If the system Rust version does not match, rustup will automatically download the correct version when building inside the repo.

---

#### 4.1.3 Python and uv (for agent-sec-core and os-skills)

Required: Python >= 3.12.

a. Alinux 4 (verified)

```bash
pip3 install uv
uv python install 3.12
```

b. Ubuntu 24.04 (verified)

```bash
sudo apt install -y pipx
pipx ensurepath
source "$HOME/.$(basename "$SHELL")rc"
pipx install uv
```

c. Other distros: uv

```bash
# Skip if uv is already installed
if command -v uv &>/dev/null; then
  echo "uv $(uv --version) already installed, skipping"
else
  curl -LsSf https://astral.sh/uv/install.sh | sh
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

#### 4.1.4 AgentSight System Dependencies (Optional)

AgentSight is an optional component that provides eBPF-based audit and observability capabilities. It is not required for core ANOLISA functionality. If you choose to build it, the following system-level dependencies are needed:

a. dnf (Alinux / Anolis OS / Fedora / RHEL / CentOS / etc.)

```bash
sudo dnf install -y clang llvm libbpf-devel elfutils-libelf-devel zlib-devel openssl-devel perl perl-IPC-Cmd
sudo dnf install -y kernel-devel-$(uname -r)
```

b. apt (Debian / Ubuntu)

```bash
sudo apt-get update -y
sudo apt-get install -y clang llvm libbpf-dev libelf-dev zlib1g-dev libssl-dev perl linux-headers-$(uname -r)
```

> Some distributions do not provide a separate perl-core package. That is expected.

c. Kernel Requirement

AgentSight requires Linux kernel >= 5.10 and BTF enabled (`CONFIG_DEBUG_INFO_BTF=y`).

---

#### 4.1.5 Version Check

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

### 4.2 Build Components

#### 4.2.1 copilot-shell

```bash
cd src/copilot-shell
make install
make build
npm run bundle
```

Artifact: `dist/cli.js`

Run options:

a. Run directly from the build directory

```bash
node dist/cli.js
```

b. Add a persistent `co` alias to your shell

```bash
make create-alias
source "$HOME/.$(basename "$SHELL")rc"
co
```

---

#### 4.2.2 os-skills

No compilation is required. Each skill is a directory containing a `SKILL.md` and optional supporting files (scripts, references, etc.). Deployment copies the entire skill directory to the target path.

Skill search paths (Copilot Shell discovers skills in the following priority order):

| Scope | Path |
|-------|------|
| Project | `.copilot/skills/` |
| User | `~/.copilot/skills/` |
| System | `/usr/share/anolisa/skills/` |

Install options:

a. Using the build script (automatic)

```bash
./scripts/build-all.sh --component skills
```

b. Manual deployment (user-level)

```bash
mkdir -p ~/.copilot/skills
find src/os-skills -name 'SKILL.md' -exec sh -c \
	'cp -rp "$(dirname "$1")" ~/.copilot/skills/' _ {} \;
```

Verify:

```bash
co /skills
```

---

#### 4.2.3 agent-sec-core (Linux only)

```bash
cd src/agent-sec-core
make build-sandbox
```

Artifact: `linux-sandbox/target/release/linux-sandbox`

Install:

```bash
sudo make install
```

---

#### 4.2.4 agentsight (Optional, Linux only)

> Note: AgentSight is an optional component. It provides eBPF-based audit and observability capabilities but is not required for core ANOLISA functionality.

```bash
cd src/agentsight
make build
```

Artifact: `target/release/agentsight`

Install:

```bash
sudo make install
```

---

### 4.3 Run Tests (Recommended)

a. Unified entry

```bash
./tests/run-all-tests.sh
./tests/run-all-tests.sh --filter shell
./tests/run-all-tests.sh --filter sec
./tests/run-all-tests.sh --filter sight
```

b. Per component

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
