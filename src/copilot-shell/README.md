# Copilot Shell

[中文版](README_CN.md)

An AI-powered terminal assistant for code understanding, task automation, and system management. Copilot Shell is a core component of [ANOLISA](../../README.md), built on top of upstream [Qwen Code](https://github.com/QwenLM/qwen-code) v0.9.0.

## Features

- **Natural Language Coding** — Describe changes in plain language to modify code, implement features, or fix bugs.
- **Code Analysis & Navigation** — Understand entire project structures and answer code-related questions.
- **Multi-Tool Orchestration** — Integrates file, shell, search, web, LSP, and MCP tools in a single session.
- **Interactive Shell** — `/bash` command to drop into an interactive shell; type `exit` to return.
- **Skill System** — Local + remote skill discovery with priority-based fallback (Project > User > Extension > Remote).
- **Hooks System** — PreToolUse events for intercepting tool calls before execution.
- **Git Workflow Automation** — Automate commits, branch creation, conflict resolution, and release notes.
- **Multi-Provider Support** — Qwen OAuth, Aliyun (BaiLian), Custom Provider (DashScope, DeepSeek, Kimi, GLM, MiniMax, or any OpenAI-compatible endpoint).
- **PTY Mode** — Full pseudo-terminal support including `sudo` commands.
- **Extensible** — Extend capabilities via MCP servers and custom skills.

## Quick Start

### Install from RPM

```bash
sudo yum install copilot-shell
```

### Install from Source

```bash
cd src/copilot-shell
make build
```

### Run

```bash
make start

# Interactive mode
cosh

# Or use alternative aliases
co
copilot
```

### Authenticate

```bash
# Qwen OAuth (free tier: 2,000 requests/day)
cosh    # follow the on-screen prompts

# API Key
cosh --auth apikey

# BaiLian / Custom Provider (OpenAI Compatible)
cosh --auth openai
```

## Architecture

Copilot Shell uses a monorepo layout with npm workspaces:

| Package               | Description                                                                |
| --------------------- | -------------------------------------------------------------------------- |
| `packages/cli`        | Terminal UI layer — input handling, command parsing, Ink/React rendering   |
| `packages/core`       | Backend core — AI model communication, prompt building, tool orchestration |
| `packages/test-utils` | Shared test utilities                                                      |

## Development

### Prerequisites

- Node.js >= 20.0.0
- npm (included with Node.js)

### Commands

```bash
make install          # Install dependencies
make build            # Build all packages
make test             # Run unit tests
make lint             # Lint with ESLint
make format           # Format with Prettier
```

### RPM Packaging

```bash
make rpm
```

## Configuration

Copilot Shell uses a layered configuration system (highest priority first):

1. Command-line arguments
2. Environment variables
3. Project settings (`.copilot/settings.json`)
4. User settings (`~/.copilot/settings.json`)
5. System settings
6. Defaults

## Documentation

- [Quick Start](docs/users/quickstart.md)
- [Contributing](CONTRIBUTING.md)

## License

Apache License 2.0 — see [LICENSE](../../LICENSE) for details.
