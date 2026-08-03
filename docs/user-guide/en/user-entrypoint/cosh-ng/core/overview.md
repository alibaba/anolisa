# Integrate Another Frontend

[中文版](../../../../zh/user-entrypoint/cosh-ng/core/overview.md)

`cosh-core` is the Agent runtime behind the interactive `cosh` terminal. It
owns provider access, the model/tool loop, hooks, Skills, MCP, extensions,
registry state, conversation persistence, and compaction.
Most users should start `cosh`; invoke cosh-core directly only to integrate a
frontend or automate a runtime control operation.

## Supported entry points

```bash
# One prompt, then exit
cosh-core --headless "Inspect disk usage; do not modify anything"

# Long-running JSONL process
cosh-core --headless

# Resume or compact a persisted conversation
cosh-core --headless --resume <session-id>
cosh-core --resume <session-id> --compact

# One provider-free registry request on stdin
cosh-core --registry
```

Without `--headless`, non-TTY stdin still selects headless mode automatically.
The interactive terminal uses a long-running headless process and the registry
protocol rather than cosh-core's direct TTY UI.

## Important options

| Option | Effect |
|---|---|
| `--headless` | Force JSONL stdin/stdout mode |
| `--model <name>` | Override the configured model |
| `--approval-mode <mode>` | Override `trust`, `auto`, `balanced`, or `strict` |
| `--allowed-tools <names>` | Bypass approval for exact registered names |
| `--tools <selection>` | Expose `default`, no tools, or a comma-separated subset |
| `--bare` | Disable project config, hooks, Skills, extensions, and session persistence |
| `--resume <id>` | Select an existing workspace-scoped conversation |
| `--compact` | Compact the selected conversation and exit |
| `--registry` | Handle one registry request and exit |
| `--enable-shell-evidence-tool` | Add bounded terminal-evidence access for cosh-shell |
| `--verbose` | Raise stderr logging verbosity |

`--allowed-tools` changes approval policy; `--tools` changes what the model can
see. Do not confuse the two.

## Runtime lifecycle

1. Resolve the workspace and layered configuration.
2. Build a runtime generation containing provider-independent tools, Skills,
   extension capabilities, and MCP connections.
3. Select/authenticate the provider.
4. Read JSONL messages and stream model/tool events.
5. Request approval or user input through control messages when required.
6. Persist the transcript and model-visible projection at safe boundaries.
7. Publish healthy registry changes immediately when idle, or defer them until
   the active run completes.

The process writes logs to stderr/file output; stdout remains protocol-only in
headless and registry modes.

## Capability map

| Capability | Reference |
|---|---|
| JSONL and registry messages | [Headless mode](headless-mode.md) |
| Providers and authentication | [Providers](providers.md) |
| Built-in, MCP, and extension tools | [Tools](tools.md) |
| MCP server setup and lifecycle | [Connect an MCP server](../mcp.md) |
| Reusable instructions | [Skills](skills.md) |
| Event policy | [Hooks](hooks.md) |
| Packaged capabilities | [Extensions](extensions.md) |
| Session configuration | [Configuration](../configuration.md) |

Protocol integrators should also read the developer
[IPC protocol reference](../../../../../developer-guide/en/cosh-ng/ipc-protocol.md).
