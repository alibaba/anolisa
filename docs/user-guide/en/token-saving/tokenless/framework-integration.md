# Tokenless Agent Integration

[中文版](../../../zh/token-saving/tokenless/framework-integration.md)

Tokenless connects to Agent products through plugins, hooks, and extensions. This guide covers
product adapters. The Python SDK and its AgentScope-specific child document live under
[Python SDK](sdk.md).

## Agent adapter support matrix

| Agent product | Value | Tool Ready | Rewrite behavior | Response delivery | TOON | Schema |
|-----------|-------|------------|------------------|-------------------|------|--------|
| cosh | `cosh` | Hard-disabled | Replaces supported shell input | Cosh-NG replaces lossless JSON results; legacy Copilot Shell passes through | Pipeline-selected for replaceable text | Lossless-only through the Common Hook |
| OpenClaw | `openclaw` | Hard-disabled | Replaces the `exec` command input | Replaces the persisted tool-result message | Off by default; opt in | — |
| Hermes | `hermes` | Hard-disabled | Blocks the first call and asks the agent to retry | Replaces the result string | Attempted after response compression | — |
| Qoder | `qoder` | Hard-disabled | Emits rewritten shell input | Replaces output through `updatedToolOutput` | Pipeline-selected for replaceable text | — |
| Claude Code | `claude-code` | Hard-disabled | Replaces Bash input | Replaces output on 2.1.121 or later; otherwise passes through | Pipeline-selected for replaceable text | — |
| Codex | `codex` | Hard-disabled | Replaces supported shell input | Keeps the original; adds context only for classified environment failures | — | — |
| DeepSeek Harness | `dsh` | — | — | Replaces an accepted single-text JSON result when the replacement is smaller | — | — |
| OpenCode | `opencode` | Hard-disabled | Replaces Bash input | Replaces tool output | Pipeline-selected for replaceable text | ✅ |
| Qwen Code | `qwencode` | Hard-disabled | Emits rewritten shell input | Passes through because the host has no replacement field | — | — |

“—” means that the capability is not available: the current adapter does not register it, or current host releases do not run it. The corresponding Tokenless CLI command may still be available.

Schema compression reaches the model path differently per host: cosh and Cosh-NG fire the `BeforeModel` hook; OpenCode compresses each tool definition through its `tool.definition` plugin hook (MCP tools do not pass through that hook); Qwen Code's manifest declares a `BeforeModel` hook, but current Qwen Code releases skip that unknown event name at registration, so the schema hook does not run there and the matrix marks it unavailable. The entry stays registered, so a future Qwen Code release that implements the event picks it up automatically.

Tool Ready remains registered by these adapters but is unconditionally hard-disabled before checking, repair, or blocking. No runtime setting can re-enable it. Post-tool failure attribution is independent.

`additionalContext` is an additive hook field. The shared hook does not place compressed copies
there because the original would remain visible and total context would grow. It uses that field
only for additive environment-error guidance. A statistics record proves that a candidate became
smaller, not by itself that the host removed the original from its model request.

## Adapter processing rules

The shared Cosh-NG, Qoder, Claude Code, and OpenCode PostTool hook sends one Protocol v2
`post_tool` request to `tokenless compress`. It declares output replacement but no trusted
agent-facing Retrieve capability. Core therefore applies only lossless JSON candidates and returns
the original for every non-`applied` disposition. It currently routes:

| Content | Current shared-hook behavior |
|---------|------------------------------|
| JSON | Lossless structural cleanup; TOON may be selected for text-capable replacement slots |
| JSON requiring string, array, or depth truncation | Rejected with `recoverability_unavailable` because the Common Hook cannot publish an authorized Retrieve tool |
| Build/test/package logs, long plain text, diff, stack trace, HTML, search results, tables, source code, unknown | Passthrough until a matching domain compressor is connected |

Content detection, the 200-character PostTool gate, tool-origin thresholds, diagnostics, TOON
selection, and final acceptance are Core policy. The hook maps host objects to v2 fields and may
skip obvious non-JSON skill files only to avoid an unnecessary subprocess.

The Common BeforeModel hook also uses Protocol v2 and declares no trusted Retrieve capability.
Core returns transformed tools only when the result is lossless. Every current `SchemaCompressor`
transformation removes or rewrites schema information, so this Common path passes tools through
unchanged, emits no schema-compression Stats rows, and never emits unrecoverable Markers. OpenCode's
separate per-tool definition path and the direct `compress-schema` command are unchanged.

Legacy OpenClaw, Hermes, and DeepSeek Harness integrations still use their dedicated response paths.
Their response thresholds and feature sets are described below; the content-aware build/log path
does not run there yet. The standalone `compress-response` command also remains the explicit JSON
cleanup interface.

For JSON response cleanup, shared and legacy adapters classify tools as follows:

| Class | Default adapter behavior |
|-------|--------------------------|
| Content retrieval, including Read/Glob/Grep/LSP/NotebookRead aliases | Skip response compression |
| Shell/exec | 65,536-character strings, 128 retained array items, depth 8 |
| Other structured tools | 1,048,576-character strings, 65,536 retained array items, depth 32 |

OpenClaw and Hermes still apply their existing adapter-side 200-character gates. In the Common Hook,
that gate now belongs to Core. TOON runs only on payloads of at least 500 characters and only when
the selected host slot accepts text; smaller payloads keep the prior candidate. The same default
minimum applies to the standalone `compress-toon` CLI and SDK TOON path, while the CLI can lower it
per call with `--min-toon-chars`. Codex and Qwen Code do not run response compression or TOON because
their current PostToolUse contracts cannot replace the original model-visible output.

The Common PreTool rewrite hook still invokes RTK directly and does not yet carry v2
`output_optimization: "rtk"` into the later PostTool process. OpenClaw has the same per-call state
gap. Their complete state migration is deferred to the adapter phase; the current PostTool request
uses `output_optimization: "none"`.

Claude Code requires version 2.1.121 or later for `updatedToolOutput`. On older or unknown versions, response compression is disabled to avoid duplicating the original. Structured tool outputs preserve their host schema and do not switch to textual TOON; JSON carried as a string can use TOON when it is smaller.

### DeepSeek Harness native processing

The DSH bundle requires Node.js 22 or later and a compatible DSH profile. Pass
all desired profile names in the same enable command, then start DSH with one
of those names:

```bash
anolisa adapter enable tokenless dsh \
  --profile web \
  --profile headless
dsh --profile web
```

`--profile` is required and repeatable. Each enable or re-enable treats its
arguments as the complete desired profile set. It removes the bundle from any
profile recorded by the prior receipt but omitted from the new command, so
always include every profile that should retain Tokenless. ANOLISA records the
selected profiles and their resolved DSH home in the adapter receipt, so later
status, disable, and re-enable operations continue to address the same profile
tree.

The plugin runs on DSH's `tools/post-execute` waterfall. It attempts
`tokenless compress-response` only for a successful result containing one text
block whose text is a JSON object or array. It replaces the content only when
the CLI returns valid JSON that is strictly shorter. Multiple blocks, images,
plain text, invalid JSON, errored results, Code Mode child executions, and the
default content-retrieval tools are not compressed. A missing, failing, or
timed-out CLI also preserves the original content. This native path does not
run the TOON second stage and has no pre-spawn minimum-size gate.

Add an override for the installed row to
`$DSH_HOME/profiles/<profile>/cordis.patch.yml`, then restart that DSH profile:

```yaml
- id: anolisa-tokenless
  config:
    responseCompressionEnabled: true
    timeoutMs: 5000
    maxBuffer: 4194304
    noStash: false
```

Later DSH patch layers replace the row's complete `config` value. The plugin
supplies defaults for omitted keys, so the override may contain only the keys
that need to differ.

| Option | Default | Behavior |
|--------|---------|----------|
| `responseCompressionEnabled` | `true` | Enables response compression. Setting it to `false` does not disable environment-error attribution. |
| `tokenlessBin` | `$TOKENLESS_BIN`, then `tokenless` | Selects the Tokenless CLI executable. A non-empty plugin value takes precedence over the environment variable. |
| `skipTools` | Content-retrieval set below | Skips compression for matching tool names. A configured array replaces the default set; an empty array skips none. Attribution remains active. |
| `shellTools` | Shell/process set below | Selects shell thresholds and the tools whose structured `value` may be interpreted for failure attribution. A configured array replaces the default set. |
| `truncateStringsAt` | Shell `65536`; other `1048576` | Overrides the maximum retained string length for every tool class. Only a positive integer is accepted. |
| `truncateArraysAt` | Shell `128`; other `65536` | Overrides the maximum retained array length for every tool class. Only a positive integer is accepted. |
| `maxDepth` | Shell `8`; other `32` | Overrides maximum JSON depth for every tool class. Only a positive integer is accepted. |
| `timeoutMs` | `3000` | Bounds one Tokenless child process in milliseconds. Only a positive integer is accepted. |
| `maxBuffer` | `2097152` | Bounds captured child-process output in bytes. Only a positive integer is accepted. |
| `agentId` | `dsh` | Sets the `--agent-id` recorded by Tokenless statistics. |
| `noStash` | `false` | Passes `--no-stash` when `true`; dropped array items are otherwise eligible for Stash storage. |

The default `skipTools` set is `Read`, `read`, `read_file`, `read_many_files`,
`Glob`, `glob`, `search_file`, `list_directory`, `list_dir`, `Grep`, `grep`,
`grep_code`, `grep_search`, `search_files`, `Lsp`, `lsp`, `NotebookRead`,
`notebook_read`, and `notebookread`.

The default `shellTools` set is `Bash`, `bash`, `Shell`, `shell`, `exec`,
`terminal`, `run_shell_command`, `run_in_terminal`, `get_terminal_output`,
`execute_command`, and `process`.

Raw DSH failures marked with `isError` may receive dependency, permission,
path, network, or package attribution for any tool. Structured output is
classified only for `shellTools`. Attribution is independent of compression,
so it remains active when compression is disabled, skipped, or produces no
smaller result. When a later waterfall listener replaces the canonical
`value`, Tokenless classifies that replacement and does not carry attribution
from the superseded result.

## Manage adapters with anolisa (recommended)

These commands require an ANOLISA component record. If Tokenless was installed
directly with YUM, record the RPM once before continuing:

```bash
sudo yum install anolisa
sudo anolisa --install-mode system adopt tokenless
```

The YUM-installed CLI is available on sudo's system path; the user-local CLI
installed by `get.agentic-os.sh` may be hidden by sudo's `secure_path`.

Run the adapter commands below as the user who owns the target Agent
configuration. A user-scoped adapter operation can discover the adopted system
package while keeping the framework mutation in that user's configuration.

### 1. Scan Agent products

```bash
anolisa adapter scan
```

If the target framework is absent, confirm that its CLI or application is installed, then scan again.

### 2. Enable one adapter

```bash
anolisa adapter enable tokenless <framework>
```

Examples:

```bash
anolisa adapter enable tokenless cosh
anolisa adapter enable tokenless openclaw
anolisa adapter enable tokenless hermes
anolisa adapter enable tokenless qoder
anolisa adapter enable tokenless claude-code
anolisa adapter enable tokenless codex
anolisa adapter enable tokenless opencode
anolisa adapter enable tokenless qwencode
anolisa adapter enable tokenless dsh \
  --profile web \
  --profile headless
```

Enable only Agent products that you use. Run and verify each product's command
separately. For DSH, include every desired profile in its single enable
command.

DeepSeek Harness is profile-scoped and therefore requires at least one
`--profile`. Each name must match one passed to `dsh --profile <profile>`; the
generic command without a profile is rejected. A later enable or re-enable
must repeat every profile that should remain registered.

For OpenClaw, anolisa first attempts a normal install and does not add an unsafe-install bypass by default. If OpenClaw rejects the plugin on its safety scan, read the reported findings. Only after accepting them, retry explicitly:

```bash
anolisa adapter enable tokenless openclaw \
  --allow-unsafe-plugin-install
```

On OpenClaw releases where the underlying bypass is unsupported or a deprecated no-op, anolisa refuses this option; follow the error's `security.installPolicy` guidance instead.

The component package may be system-scoped while the adapter receipt remains
user-scoped. Use `sudo` only when the target framework configuration and its
adapter receipt are intentionally owned by root.

### 3. Check status

```bash
anolisa adapter status tokenless
anolisa doctor tokenless
```

Restart the target agent CLI or IDE afterwards. A running session normally does not load a newly installed hook or plugin dynamically.

### 4. Disable

```bash
anolisa adapter disable tokenless <framework>
```

Disable the adapter with the same user that enabled it. A root-owned receipt is
the exception and requires `sudo` for both operations.

Restart the target agent after disabling. All enabled adapters must be released before Tokenless can be uninstalled.

## Manual integration after npm installation

The npm postinstall script attempts to copy adapter resources under:

```text
~/.local/share/anolisa/adapters/tokenless/
```

Confirm that this directory exists. Adapter copying is supplementary and fails open with a warning; a successful binary install can therefore exist without this copy. If it is absent, review the npm postinstall warning and prefer an anolisa-managed installation.

An npm install does not create an anolisa component installation record, so do not assume that `anolisa adapter enable` can manage it. OpenClaw, Hermes, Qoder, Claude Code, Codex, OpenCode, and Qwen Code provide their own install scripts:

```bash
bash ~/.local/share/anolisa/adapters/tokenless/<framework>/scripts/install.sh
```

For example:

```bash
bash ~/.local/share/anolisa/adapters/tokenless/claude-code/scripts/install.sh
bash ~/.local/share/anolisa/adapters/tokenless/opencode/scripts/install.sh
```

Uninstall the same adapter with:

```bash
bash ~/.local/share/anolisa/adapters/tokenless/<framework>/scripts/uninstall.sh
```

The scripts call the framework's own plugin or extension mechanism. Follow their restart instructions. If a script is missing, fails, or reports an incompatible framework version, prefer an anolisa-managed installation.

The OpenClaw install script invokes `plugins install` with `--dangerously-force-unsafe-install` because the plugin launches the `tokenless` and `rtk` binaries through Node.js child-process APIs. Review the installed adapter source and your OpenClaw policy before running it. If that policy does not permit the override, do not install the plugin.

### npm with cosh

cosh uses an Extension directory and does not provide a separate `scripts/install.sh`. Copy the npm-installed shared resources into the user Extension directory:

```bash
mkdir -p ~/.copilot-shell/extensions/tokenless
cp -R ~/.local/share/anolisa/adapters/tokenless/common/hooks \
  ~/.local/share/anolisa/adapters/tokenless/common/commands \
  ~/.local/share/anolisa/adapters/tokenless/common/cosh-extension.json \
  ~/.copilot-shell/extensions/tokenless/
```

Restart cosh afterwards. Before removing it, exit cosh and confirm that the target directory is the Tokenless Extension created by this npm installation.

## Agent adapter activation notes

### cosh

Extensions are discovered at startup. Restart cosh, run a shell-tool task, and inspect `tokenless stats list`.

### OpenClaw

The install script uses OpenClaw's unsafe-install override as described above. Restart the gateway after accepting and installing the plugin. Response compression and RTK rewriting default to enabled in the plugin code; TOON defaults to disabled. The plugin's Tool Ready option currently has no effect because the underlying check is hard-disabled.

### Hermes

The plugin takes effect in a new Hermes session. Restart Hermes and run a shell-tool task.

### Qoder

Qoder IDE and qodercli may cache plugin configuration. Fully restart the IDE after enabling or upgrading. If an old hook path is reported, see [Qoder plugin cache issue](troubleshooting.md#qoder-plugin-cache-issue).

### Claude Code

The marketplace plugin takes effect after restarting Claude Code. The install script may also offer a plugin refresh command.

### Codex

The plugin loads in a new Codex session. Close the old session and start a new one before verifying behavior. Codex PostToolUse cannot replace or suppress the original output, so the plugin does not append compressed content or record response-compression candidates. It adds context only for classified environment failures. Actual first-pass savings come from RTK rewriting supported shell commands before execution.

### DeepSeek Harness

The native bundle loads when the selected DSH profile starts. After enabling
or changing its profile patch, restart `dsh --profile <profile>`, run a tool
that returns compressible JSON, and inspect `tokenless stats list`. Disable the
adapter with `anolisa adapter disable tokenless dsh`; the receipt already
records the profile names, so disable does not accept another `--profile`.

### OpenCode

OpenCode discovers global local plugins at startup. Use the bundled Tokenless lifecycle script described above, restart OpenCode after installation or removal, then run a tool call and inspect `tokenless stats list`. The script resolves the configuration directory from `TOKENLESS_OPENCODE_CONFIG_DIR`, then `OPENCODE_CONFIG_DIR`, then `XDG_CONFIG_HOME/opencode`, and finally `~/.config/opencode`. Installation creates only `plugins/tokenless.js` as a managed symlink and refuses to replace an unrelated file at that path.

### Qwen Code

The extension loads in a new Qwen Code session. Restart and run one tool call to verify it.

## AgentScope framework integration

AgentScope is the second Python SDK layer, not a product adapter. Its complete build, version,
attachment, configuration, and validation guidance now lives in
[AgentScope SDK integration](sdk/agentscope.md). This heading remains as a compatibility pointer for
existing links.

## Verify an Agent adapter

For an Agent adapter, do not treat a zero install exit code as the only success criterion. At
minimum, run:

```bash
tokenless --version
anolisa adapter status tokenless
tokenless stats list --limit 5
```

Then execute a tool task with visible output in the target agent. If `stats list` remains empty, follow [No statistics appear after enabling the adapter](troubleshooting.md#no-statistics-appear-after-enabling-the-adapter).

## Related documents

- [Quick Start](QUICKSTART.md)
- [Python SDK](sdk.md)
- [AgentScope SDK integration](sdk/agentscope.md)
- [Measuring savings](measuring-savings.md)
- [Configuration and data privacy](configuration-and-privacy.md)
- [Troubleshooting](troubleshooting.md)
