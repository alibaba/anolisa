# cosh-ng User Guide

[中文版](../../../zh/user-entrypoint/cosh-ng/README.md)

cosh-ng is an AI-native Linux terminal that keeps normal Shell work and Agent tasks together. Start with the quick start, then use the task-based links below for the feature or command you need.

## Start here

- [Quick start](QUICKSTART.md) — install cosh-ng and run a first task.
- [Model providers](core/providers.md) — configure authentication and select a provider.
- [Configuration](configuration.md) — review files, settings, and precedence.
- [Supported platforms](supported-distros.md) — check package and service backends.

## Work in the terminal

| Goal | Read next |
|---|---|
| Run Shell commands and natural-language tasks together | [Interactive terminal](shell/overview.md) |
| Choose when Agent tool calls require confirmation | [Tool approval](shell/approval.md) |
| Resume or compact a conversation | [Session recovery](shell/session-recovery.md) |
| Learn slash commands and keyboard behavior | [Interactive behavior](shell/interactive-mode.md) |

## Add capabilities

| Goal | Read next |
|---|---|
| Share instructions across a project or team | [Skills](core/skills.md) |
| Connect tools from a local process or remote service | [Connect an MCP server](mcp.md) |
| Bundle Skills, Hooks, settings, and tools | [Extensions](core/extensions.md) |
| Run checks around Agent lifecycle events | [Hooks](core/hooks.md) |

## Manage system operations

Use read-only commands first. Add `--dry-run` to a supported package or service mutation before making a change; these operations usually need root privileges.

| Goal | Read next |
|---|---|
| Find, install, or remove packages | [Package management](cli/package-management.md) |
| Inspect or change systemd services | [Service management](cli/service-management.md) |
| Save, compare, restore, or clean workspace snapshots | [Workspace checkpoints](cli/checkpoint.md) |
| Check policy decisions and audit events | [Security audit](cli/audit.md) |

## Integrate and automate

- [Structured OS CLI](cli/overview.md) — command domains and safe automation patterns.
- [Output format](output-format.md) — the `CoshResponse<T>` success and error envelope.
- [Headless mode](core/headless-mode.md) — JSONL integration for other frontends.
- [Agent tools](core/tools.md) — tool boundaries and approval behavior.
