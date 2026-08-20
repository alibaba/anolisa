# Tokenless Agent and Framework Integration

[中文版](../../../zh/token-saving/tokenless/framework-integration.md)

Tokenless has two integration layers. Agent adapters connect the installed binaries to concrete
Agent products through plugins, hooks, and extensions. AgentScope support is instead an in-process
Python framework package that application developers install and register explicitly.

## Agent adapter support matrix

| Agent product | Value | Tool Ready | Rewrite behavior | Response delivery | TOON | Schema |
|-----------|-------|------------|------------------|-------------------|------|--------|
| cosh | `cosh` | Hard-disabled | Replaces supported shell input | Cosh-NG replaces the response; legacy Copilot Shell appends context | Attempted after response compression | ✅ |
| OpenClaw | `openclaw` | Hard-disabled | Replaces the `exec` command input | Replaces the persisted tool-result message | Off by default; opt in | — |
| Hermes | `hermes` | Hard-disabled | Blocks the first call and asks the agent to retry | Replaces the result string | Attempted after response compression | — |
| Qoder | `qoder` | Hard-disabled | Emits rewritten shell input | Emits `additionalContext` | Attempted after response compression | — |
| Claude Code | `claude-code` | Hard-disabled | Replaces Bash input | Replaces output on 2.1.121 or later; otherwise passes through | Used only when the replacement can remain text | — |
| Codex | `codex` | Hard-disabled | Replaces supported shell input | Keeps the original and adds analysis or a compressed alternative | Used to build that alternative | — |
| DeepSeek Harness | `dsh` | — | — | Replaces an accepted single-text JSON result when the replacement is smaller | — | — |
| OpenCode | `opencode` | Hard-disabled | Replaces Bash input | Replaces tool output | Attempted after response compression | ✅ |
| Qwen Code | `qwencode` | Hard-disabled | Emits rewritten shell input | Emits `additionalContext` | Attempted after response compression | ✅ |

“—” means that the current adapter does not register that capability. The corresponding Tokenless CLI command may still be available.

Tool Ready remains registered by these adapters but is unconditionally hard-disabled before checking, repair, or blocking. No runtime setting can re-enable it. Post-tool failure attribution is independent.

`additionalContext` is an additive hook field. The Tokenless source does not remove the original result on those paths; the final treatment also depends on the host implementation. A statistics record proves that a candidate became smaller, not that the host removed the original from its model request.

OpenCode currently uses the bundled lifecycle scripts documented below and is not registered with
the `anolisa adapter enable` driver set in this release.

## Adapter processing rules

The standalone `compress-response` defaults are not the defaults used by most adapters. Shared adapters classify tools as follows:

| Class | Default adapter behavior |
|-------|--------------------------|
| Content retrieval, including Read/Glob/Grep/LSP/NotebookRead aliases | Skip response compression |
| Shell/exec | 65,536-character strings, 128 retained array items, depth 8 |
| Other structured tools | 1,048,576-character strings, 65,536 retained array items, depth 32 |

The shared response hook, OpenClaw, and Hermes skip inputs shorter than 200 characters. Codex skips inputs shorter than 500 characters; it includes compressed content only for inputs of at least 4,000 characters and otherwise adds diagnostics or a summary. Skill-like text with YAML frontmatter is also skipped by the shared paths. The TOON encoding step only runs on payloads of at least 500 characters (the current implementation threshold, which may be adjusted later); smaller payloads keep the compressed form, because TOON savings on small JSON are negligible. The threshold applies to every TOON-capable pipeline: the shared response hook, the standalone TOON hook, Codex, OpenClaw, and Hermes.

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

OpenCode uses its bundled install script under
[Manual integration after npm installation](#manual-integration-after-npm-installation).

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

An npm install does not create an anolisa component installation record, so do not assume that `anolisa adapter enable` can manage it.

### Quick setup with `tokenless init`

After installing the npm package or building from source, run `tokenless init` to detect installed agent frameworks and guide adapter installation:

```bash
# Scan and display all framework statuses
tokenless init --list

# Install a specific adapter
tokenless init --framework claude-code

# Install all installable adapters (non-interactive)
tokenless init --all

# Interactive selection (when stdin is a terminal)
tokenless init
```

`tokenless init` reads the adapter manifest, runs each framework's `detect.sh`, and reports whether the adapter is ready, installable, or missing prerequisites — then runs `install.sh` for the selected framework.

### Manual install scripts

OpenClaw, Hermes, Qoder, Claude Code, Codex, OpenCode, and Qwen Code provide their own install scripts:

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

The plugin loads in a new Codex session. Close the old session and start a new one before verifying statistics. Its PostToolUse hook is additive: use statistics as candidate-compression telemetry, not as proof that the original Codex tool output left the prompt.

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

The Python package supports AgentScope 1.0.11 through 1.0.x and AgentScope
2.0.x. Choose the attachment point for the installed framework version:

| AgentScope version | Supported entry point |
|---|---|
| 1.0.11 through 1.0.x | Tokenless Toolkit plus `install(..., session_id=...)` |
| 2.0.0 | Direct Agent construction with `integration.tools` and `integration.middlewares` |
| 2.0.1 through 2.0.x | Direct Agent construction or App through `integration.app_options()` |

The native `anolisa-tokenless` runtime wheel and AgentScope integration wheel
are not currently published to a Python package index. Build and install both
same-version wheels from a source checkout:

```bash
make python-wheel agentscope-wheel
python -m pip install \
  target/wheels/anolisa_tokenless-*.whl \
  target/wheels/anolisa_tokenless_agentscope-*.whl
```

The native wheel also exposes the same read-only statistics capabilities as
the CLI through typed Python values:

```python
from anolisa_tokenless import TokenlessStats

stats = TokenlessStats("/absolute/path/to/tenant-tokenless-data")

status = stats.status
summary = stats.summary()
recent = stats.list(limit=20)
record = stats.show(recent[0].id)
session_diff = stats.diff(session_id="conversation-id")
comparison = stats.compare("baseline-session", "tokenless-session")
```

`TokenlessSdk.stats` lazily returns a client bound to that SDK's data directory.
Token counts are estimates and only operations with positive savings are
recorded. `show()` and record/tool-use `diff()` results may contain sensitive
tool input and output from `stats.db`; summary, list, and comparison results do
not return stored content. The API cannot clear data or change recording
settings. Read-only describes those public operations: opening the client
follows CLI initialization and may create or migrate `stats.db`, so the data
directory must be writable. `limit=None` for summary or comparison reads at most
the newest 10,000 records. Session and tool-use diffs also read at most the
newest 10,000 matching records. For a meaningful comparison, pass a dry-run
baseline session first and an active Tokenless session second.

Both major versions use `TokenlessAgentScope` and `TokenlessConfig`; only the
final attachment step differs. AgentScope 1.x uses a Tokenless Toolkit whose
regular and MCP registration paths also cover tools added after construction:

```python
from agentscope.agent import ReActAgent
from tokenless_agentscope import TokenlessAgentScope, TokenlessConfig

integration = TokenlessAgentScope(
    TokenlessConfig(
        mode="balanced",
        data_dir="/absolute/path/to/tenant-tokenless-data",
    ),
)
toolkit = integration.create_toolkit()
toolkit.register_tool_function(application_tool)
agent = ReActAgent(..., toolkit=toolkit)
integration.install(agent, session_id="conversation-id")
```

In AgentScope 2.x, pass the retrieval Tool and middleware when constructing the
Toolkit and Agent. This works from 2.0.0 and does not depend on mutable Toolkit
APIs introduced in later patch releases:

```python
from agentscope.agent import Agent
from agentscope.tool import Toolkit
from tokenless_agentscope import TokenlessAgentScope, TokenlessConfig

integration = TokenlessAgentScope(
    TokenlessConfig(
        mode="balanced",
        data_dir="/absolute/path/to/tenant-tokenless-data",
        # retrieve_tool_name="tenant_tokenless_retrieve",
    ),
)
toolkit = Toolkit(tools=[*application_tools, *integration.tools])

agent = Agent(
    ...,
    toolkit=toolkit,
    middlewares=integration.middlewares,
)
```

AgentScope App is supported from 2.0.1. `app_options()` derives an isolated
Tokenless data directory for every user/agent/session below the configured
absolute base directory:

```python
from agentscope.app import create_app
from tokenless_agentscope import TokenlessAgentScope, TokenlessConfig

integration = TokenlessAgentScope(
    TokenlessConfig(data_dir="/srv/tokenless-tenants"),
)
app = create_app(..., **integration.app_options())
```

Set a unique `retrieve_tool_name` in `TokenlessConfig` if the application
already defines `tokenless_retrieve`; App assembly does not expose other tools
to its factory for a preflight collision check. AgentScope 2.0.0 does not
provide App-level Agent middleware or Tool injection, so it supports direct
Agent construction only. The existing `TokenlessMiddleware` 2.x API remains
available for compatibility; new code should use `TokenlessAgentScope` to
avoid patch-specific Toolkit mutation and automatic Tool collection behavior.

Choose a mode according to how much inline truncation the application accepts:

| Mode | Read/Glob/Grep | Other tools |
|------|----------------|-------------|
| `conservative` | Compress | 1 MiB strings, 65,536 array items, depth 32 |
| `balanced` (default) | Skip | Shell: 65,536 / 128 / depth 8; others: conservative limits |
| `aggressive` | Skip | CLI defaults: 4,096 / 32 / depth 8 |

The integration passes intermediate streaming chunks through unchanged, preserves framework
objects, and transforms only copied call arguments and final model-visible text. Tokenless keeps
the original whenever an optimization fails or does not make the UTF-8 result strictly smaller.
`DataBlock` values are never changed.

The integration also exposes a retrieval Tool named `tokenless_retrieve` by
default. It is published to the model only when a marker is visible and returns
content only for an exact 24-character hexadecimal hash retained in that
session's marker set. The Tool is permanently excluded from compression. This
narrow permission still depends on storage isolation: pass a
separate absolute `data_dir` for every user or tenant. If `data_dir` is omitted,
`TOKENLESS_DATA_DIR` is only a process-wide fallback and must not be shared by
multiple tenants. Retrieval does not work across nodes. Stash entries expire
after the current fixed one-hour TTL, so the Agent should retrieve necessary
content before that boundary.

Both adapters enable schema compression, RTK command rewriting, response compression, TOON,
retrieval, environment-error guidance, and per-call attribution. The platform wheel contains RTK
and links TOON directly, so it does not search for system helper binaries. Tool Ready remains
hard-disabled.

## Verify the actual integration

For an Agent adapter, do not treat a zero install exit code as the only success criterion. At
minimum, run:

```bash
tokenless --version
anolisa adapter status tokenless
tokenless stats list --limit 5
```

Then execute a tool task with visible output in the target agent. If `stats list` remains empty, follow [No statistics appear after enabling the adapter](troubleshooting.md#no-statistics-appear-after-enabling-the-adapter).

For the AgentScope framework package, validate the two wheels and the declared AgentScope version
range from a source checkout with:

```bash
make test-agentscope-integration
```

Then exercise one successful, compressible tool response in the application and confirm that the
middleware returns the smaller result and that `tokenless_retrieve` can recover marker-scoped
content from the same `data_dir`.

## Related documents

- [Quick Start](QUICKSTART.md)
- [Measuring savings](measuring-savings.md)
- [Configuration and data privacy](configuration-and-privacy.md)
- [Troubleshooting](troubleshooting.md)
