# Extensions

[中文版](../../../../zh/user-entrypoint/cosh-ng/core/extensions.md)

Extensions package one or more Skills, hooks, MCP servers, settings, context
files, or Agent definitions. Manage them from the interactive terminal so cosh
can show capability changes and request consent before activating executable
behavior.

## Common commands

```text
/extensions list
/extensions info <name>
/extensions doctor [name]

/extensions install ./extension
/extensions install https://example.com/extension.git --ref main
/extensions link ./extension
/extensions update <name>
/extensions update --all
/extensions uninstall <name>

/extensions enable <name>
/extensions disable <name>
/extensions reload
```

Use `install` for a managed copy and `link` for local extension development.
HTTPS Git sources may select a ref. Run `/extensions help` for the exact syntax
supported by the installed version.

## Consent flow

Install, link, uninstall, and capability-changing updates may return a pending
operation instead of mutating immediately:

```text
/extensions operation <operation-id>
/extensions consent <operation-id>
/extensions cancel <operation-id>
```

Review the source, version, and capability diff before consent. Capability
fingerprints include executable hook commands and their declared environment;
changing either requires fresh consent.

## Settings

```text
/extensions settings list <name> [--scope user|workspace]
/extensions settings get <name> <key> [--scope user|workspace]
/extensions settings set <name> <key> <value> --scope user
/extensions settings unset <name> <key> --scope workspace
```

Sensitive values are stored in the operating-system secret store and rendered
as `[redacted]`. Workspace settings apply only to an already trusted project.

## Source priority

System extensions normally live under `/usr/share/anolisa/extensions/`; user
extensions live under `~/.copilot-shell/extensions/`. When both provide the
same identity, cosh reports the ambiguity and can select an explicit source:

```text
/extensions select-source <name> user
/extensions select-source <name> system
```

## Activation model

cosh builds a candidate registry generation before activation. An unhealthy
candidate is rejected without replacing the current generation. A healthy
candidate activates immediately when no Agent run is active; otherwise one
reload waits for the next safe point, and the active run stays pinned to its
original generation.

Linked extensions are checked for source drift. `/extensions doctor` reports
invalid manifests, stale consent, missing files, source conflicts, and runtime
load failures.

## Capability boundaries

- Local stdio MCP tools use the full
  `<extension>/mcp/<server>/<tool>` namespace and retain normal approval.
- Hook `env` values apply only to the hook child process; they never mutate the
  host process.
- Extension context is bounded and inserted after project context.
- Agent definitions are validated and listed, but report `executable=false`
  until the unified subagent executor is available.
- Disabling an extension removes its runtime capabilities at the next healthy
  generation boundary; it does not delete the installed package.

Authors can use `/extensions new <path> --template <name>` to create a current
manifest scaffold, where `<name>` is `minimal`, `skill`, `hook`, `mcp`,
`context`, or `agent`; then validate it with `/extensions doctor`.
