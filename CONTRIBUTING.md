# Contributing to ANOLISA

We welcome contributions! This document covers the basics of contributing to the project.

> **Done coding?** Ask your AI assistant: *"Read AGENT.md and help me generate a commit message and PR description."*

## Development Environment

### Prerequisites

- **Node.js** >= 20.0.0 (for copilot-shell)
- **Python** >= 3.12.0 (for os-skills)
- **Rust** stable toolchain (for agent-sec-core and agentsight, Linux only)
- **uv** (Python package manager for os-skills)
- **clang** & **libbpf** (for compiling agentsight eBPF C code)

### Getting Started

```bash
# Clone the repository
git clone https://github.com/alibaba/anolisa.git
cd anolisa

# Quick build: install deps + build + install to system (recommended)
./scripts/build-all.sh

# Build selected components only
./scripts/build-all.sh --component cosh --component sec-core

# Run unified tests
./tests/run-all-tests.sh
```

For a full breakdown of build options and dependency installation, see [docs/BUILDING.md](docs/BUILDING.md).

### Package-Specific Development

Each component has its own build workflow:

- **copilot-shell**:
  ```bash
  cd src/copilot-shell
  make deps   # npm install + husky hooks
  make build
  ```
  > `make deps` sets up [husky](https://typicode.github.io/husky/) pre-commit hooks that run Prettier and ESLint on staged files. Use `make deps-ci` in CI to skip hook installation.

- **os-skills**: `cd src/os-skills` — skill definitions are static assets, no compilation needed

- **agent-sec-core** (Linux only): `cd src/agent-sec-core && make build-sandbox`

- **agentsight** (Linux only, optional): `cd src/agentsight && make build`

## Build & Test Commands

```bash
# Unified build (recommended — handles deps, build, and install automatically)
./scripts/build-all.sh                                        # all default components
./scripts/build-all.sh --no-install                           # build only, skip system install
./scripts/build-all.sh --ignore-deps                          # skip dep installation
./scripts/build-all.sh --component cosh --component sec-core  # selected components

# Unified test runner
./tests/run-all-tests.sh             # all components
./tests/run-all-tests.sh --filter shell   # copilot-shell only
./tests/run-all-tests.sh --filter sec     # agent-sec-core only

# Per-component
cd src/copilot-shell && make lint && make test
cd src/agent-sec-core && pytest tests/integration-test/ tests/unit-test/
cd src/agentsight && cargo test
```

## Contribution Process

1. **Open an issue first** - Discuss your proposed change before writing code.
2. **Fork and branch** - Create a feature branch from `main`.
3. **Make your changes** - Follow existing code style and conventions.
4. **Write tests** - Ensure adequate test coverage for your changes.
5. **Run preflight checks** — lint and test the affected component before submitting:
   ```bash
   # copilot-shell
   cd src/copilot-shell && make lint && make test
   # agent-sec-core
   cd src/agent-sec-core && pytest tests/
   # agentsight
   cd src/agentsight && cargo clippy -- -D warnings && cargo test
   ```
6. **Submit a PR** - Link it to the issue and provide a clear description.

### Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(cosh): add --json flag to config command
fix(sec-core): handle sandbox escape edge case
docs(docs): update installation guide
```

**scope is mandatory** and must be one of the following:

| Scope | Covers |
|-------|--------|
| `cosh` | `src/copilot-shell/` |
| `sec-core` | `src/agent-sec-core/` |
| `skill` | `src/os-skills/` |
| `sight` | `src/agentsight/` |
| `ci` | `.github/workflows/` |
| `docs` | `docs/` or documentation updates |
| `deps` | Dependency version bumps (lock files) |
| `chore` | Other maintenance (config, scripts, tooling) |

### Branch Naming

Internal contributors should follow this convention:

```
feature/<scope>/<short-desc>    e.g. feature/cosh/json-output
fix/<scope>/<short-desc>        e.g. fix/sec-core/sandbox-escape
hotfix/<scope>/<short-desc>     e.g. hotfix/skill/broken-load
```

**Fork contributors**: branch naming is free — CI will only issue a suggestion, not block your PR.

### CI Checks Explained

When you open a PR, the following checks run automatically:

| Check | Level | How to fix |
|-------|-------|------------|
| Commit scope missing | **Error** (blocks merge) | Add `(scope)` to every commit message, e.g. `fix(cosh): ...` |
| Commit scope not in allowed list | Warning | Use one of the scopes above: `cosh`, `sec-core`, `skill`, `sight`, `ci`, `docs`, `deps`, `chore` |
| PR title format | Warning | Follow `type(scope): description` — same as commit messages |
| Branch name convention | Warning | Follow `feature/<scope>/<desc>` — not required for forks |
| PR not linked to an issue | Warning | Add `closes #<n>` to your PR description, or `no-issue: <reason>` |
| CI tests fail | **Error** (blocks merge) | Fix the failing tests before requesting review |

## Code Style

- **TypeScript**: ESLint + Prettier (configured in copilot-shell)
- **Python**: Ruff + Black (configured in os-skills)
- **Rust**: `cargo fmt` + `cargo clippy`

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
