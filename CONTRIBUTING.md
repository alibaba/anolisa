# Contributing to ANOLISA

We welcome contributions! This document covers the basics of contributing to the project.

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

# Run unified tests
./tests/run-all-tests.sh

# Install copilot-shell dependencies locally for dev
cd src/copilot-shell
npm ci
npm run bundle
```

> **Note:** `npm ci` (or `npm install`) automatically runs the `prepare` script, which initializes [husky](https://typicode.github.io/husky/) pre-commit hooks. On each commit, [lint-staged](https://github.com/lint-staged/lint-staged) will run **Prettier** and **ESLint** on your staged files. If you need to bypass the hooks in an emergency, use `git commit --no-verify`.

### Package-Specific Development

Each package has its own development workflow:

- **copilot-shell**: See [src/copilot-shell/CONTRIBUTING.md](src/copilot-shell/CONTRIBUTING.md)
- **os-skills**: `cd src/os-skills` — Skill definitions loaded via Skill-OS framework
- **agent-sec-core**: `cd src/agent-sec-core && make build-sandbox` (Linux only)
- **agentsight**: `cd src/agentsight && cargo build` (requires libbpf)

## Workspace Commands

```bash
# Using root Makefile
make install          # Install all dependencies
make build            # Build copilot-shell
make build-sec        # Build agent-sec-core sandbox (Linux)
make build-sight      # Build agentsight (Linux)
make test             # Run tests (with filters available)
make lint             # Lint copilot-shell
make format           # Format all sub-projects
make rpm              # Build all RPM packages
```

## Contribution Process

1. **Open an issue first** - Discuss your proposed change before writing code.
2. **Fork and branch** - Create a feature branch from `main`.
3. **Make your changes** - Follow existing code style and conventions.
4. **Write tests** - Ensure adequate test coverage for your changes.
5. **Run preflight checks** - `make lint && make test` before submitting.
6. **Submit a PR** - Link it to the issue and provide a clear description.

### Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(cli): add --json flag to config command
fix(core): handle empty response from API
docs: update installation guide
```

## Code Style

- **TypeScript**: ESLint + Prettier (configured in copilot-shell)
- **Python**: Ruff + Black (configured in os-skills)
- **Rust**: `cargo fmt` + `cargo clippy`

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
