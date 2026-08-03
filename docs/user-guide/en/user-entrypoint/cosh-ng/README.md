# cosh-ng User Guide

[中文版](../../../zh/user-entrypoint/cosh-ng/README.md)

cosh-ng keeps everyday Shell work and Agent tasks in one Linux terminal. This
guide starts with what you want to accomplish, then points to command and
integration references when you need more detail.

## Start here

- [Install and run your first task](QUICKSTART.md)
- [Choose a model provider and sign in](core/providers.md)
- [Review configuration and file precedence](configuration.md)
- [Check supported platforms](supported-distros.md)

## Work in the terminal

| What you want to do | Read next |
|---|---|
| Mix Shell commands with natural-language tasks | [Interactive terminal](shell/overview.md) |
| Decide which Agent actions need confirmation | [Tool approval](shell/approval.md) |
| Resume earlier work | [Session recovery](shell/session-recovery.md) |
| Keep a long conversation within the model window | [Session compaction](shell/session-compaction.md) |
| Understand slash commands and keyboard behavior | [Interactive behavior](shell/interactive-mode.md) |

## Add reusable capabilities

| What you want to add | Read next |
|---|---|
| Instructions shared by a team or project | [Skills](core/skills.md) |
| Tools from a local process or remote service | [Connect an MCP server](mcp.md) |
| Packaged Skills, Hooks, settings, and tools | [Extensions](core/extensions.md) |
| Checks that run around Agent events | [Hooks](core/hooks.md) |

## Manage system operations

Start with read-only commands or `--dry-run` whenever the operation supports
it. Package and service changes usually require root privileges.

| Task | Read next |
|---|---|
| Install, remove, or find packages | [Package management](cli/package-management.md) |
| Inspect or change systemd services | [Service management](cli/service-management.md) |
| Save and restore workspace state | [Workspace checkpoints](cli/checkpoint.md) |
| Review policy and audit records | [Security audit](cli/audit.md) |

## Integrate and automate

- [Structured OS CLI](cli/overview.md) explains the stable JSON interface.
- [Output format](output-format.md) documents success and error envelopes.
- [Headless mode](core/headless-mode.md) is for other frontends and JSONL
  integrations.
- [Agent tools](core/tools.md) documents tool boundaries and approval behavior.
