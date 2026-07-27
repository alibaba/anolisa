# cosh-core Overview

cosh-core is the AI Agent runtime core of cosh-ng. It provides a headless JSONL backend integrating LLM providers, hook system, tool execution, skill management, and session persistence.

## Positioning

cosh-core serves as the backend engine for cosh-shell. cosh-shell communicates with the cosh-core process via stdin/stdout (JSONL protocol). cosh-core can also be used independently:

- **Single prompt mode** — Pass in a prompt directly, exit after execution
- **Headless mode** — Long-running process, continuously receiving JSONL messages
- **Registry mode** — Process registry requests only, then exit

## Run Modes

```bash
# Single prompt (requires explicit --headless)
cosh-core --headless "Check system load"

# Long-running headless mode (receives JSONL via stdin)
cosh-core --headless

# Registry mode
cosh-core --registry

# Override model
cosh-core --headless --model qwen-max "Analyze this code"

# Override approval mode
cosh-core --headless --approval-mode trust "Install nginx"

# Resume session
cosh-core --headless --resume <session-id>
```

## CLI Arguments

| Argument | Description |
|----------|-------------|
| `--headless` | Force headless JSONL mode |
| `--model <name>` | Override configured model |
| `--approval-mode <mode>` | Override approval mode (trust/auto/balanced/strict) |
| `--allowed-tools <tools>` | Comma-separated list of auto-approved tools |
| `--resume <session-id>` | Resume an existing session |
| `--verbose` | Increase log verbosity |
| `--registry` | Registry mode |
| `--enable-shell-evidence-tool` | Enable terminal output evidence tool |

## Core Capabilities

| Capability | Description | Documentation |
|-----------|-------------|---------------|
| LLM Providers | OpenAI compatible / Aliyun SysOM | [providers.md](providers.md) |
| Hook System | 8 event points, extensible | [hooks.md](hooks.md) |
| Tool Execution | Built-in tools + custom tools | [tools.md](tools.md) |
| Skill Management | Markdown skill definitions | [skills.md](skills.md) |
| Extension Loading | cosh-extension.json | [extensions.md](extensions.md) |
| Session Persistence | JSON format session storage | — |

## Architecture Overview

```
stdin (JSONL)                stdout (JSONL)
     │                            ▲
     ▼                            │
┌────────────────────────────────────────┐
│              cosh-core                 │
│  ┌──────┐  ┌──────────┐  ┌──────────┐  │
│  │ Auth │  │ Provider │  │  Tools   │  │
│  └──────┘  └──────────┘  └──────────┘  │
│  ┌──────┐  ┌──────────┐  ┌──────────┐  │
│  │Hooks │  │ Session  │  │  Skills  │  │
│  └──────┘  └──────────┘  └──────────┘  │
└────────────────────────────────────────┘
```

## Authentication Flow

If no API key is configured, cosh-core sends an `auth_required` control request on startup:

1. Core sends `AuthRequired`, listing available authentication providers
2. Shell (or external client) displays authentication UI
3. User selects a provider and fills in credentials
4. Shell sends back `ControlResponse` containing credentials
5. Core applies credentials, optionally persists to config.toml
6. Core sends `auth_ok` status, begins normal operation
