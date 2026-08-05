# Tokenless Framework Integration

[中文版](../../../zh/token-saving/tokenless/framework-integration.md)

Tokenless uses adapters to connect compression, command rewriting, and environment checks to an agent. Installing Tokenless provides the binaries and adapter resources; the target agent calls them automatically only after its adapter is enabled.

## Support matrix

| Framework | Value | Tool Ready | Rewrite behavior | Response delivery | TOON | Schema |
|-----------|-------|------------|------------------|-------------------|------|--------|
| cosh | `cosh` | ✅ | Replaces supported shell input | Cosh-NG replaces the response; legacy Copilot Shell appends context | Attempted after response compression | ✅ |
| OpenClaw | `openclaw` | ✅ | Replaces the `exec` command input | Replaces the persisted tool-result message | Off by default; opt in | — |
| Hermes | `hermes` | ✅ | Blocks the first call and asks the agent to retry | Replaces the result string | Attempted after response compression | — |
| Qoder | `qoder` | ✅ | Emits rewritten shell input | Emits `additionalContext` | Attempted after response compression | — |
| Claude Code | `claude-code` | ✅ | Replaces Bash input | Replaces output on 2.1.121 or later; otherwise passes through | Used only when the replacement can remain text | — |
| Codex | `codex` | ✅ | Replaces supported shell input | Keeps the original and adds analysis or a compressed alternative | Used to build that alternative | — |
| OpenCode | `opencode` | ✅ | Replaces Bash input | Replaces tool output | Attempted after response compression | ✅ |
| Qwen Code | `qwencode` | ✅ | Emits rewritten shell input | Emits `additionalContext` | Attempted after response compression | ✅ |

“—” means that the current adapter does not register that capability. The corresponding Tokenless CLI command may still be available.

`additionalContext` is an additive hook field. The Tokenless source does not remove the original result on those paths; the final treatment also depends on the host implementation. A statistics record proves that a candidate became smaller, not that the host removed the original from its model request.

OpenCode currently uses the bundled lifecycle scripts documented below. It is not registered with the `anolisa adapter enable` driver set in this release.

## Adapter processing rules

The standalone `compress-response` defaults are not the defaults used by most adapters. Shared adapters classify tools as follows:

| Class | Default adapter behavior |
|-------|--------------------------|
| Content retrieval, including Read/Glob/Grep/LSP/NotebookRead aliases | Skip response compression |
| Shell/exec | 65,536-character strings, 128 retained array items, depth 8 |
| Other structured tools | 1,048,576-character strings, 65,536 retained array items, depth 32 |

The shared response hook, OpenClaw, and Hermes skip inputs shorter than 200 characters. Codex skips inputs shorter than 500 characters; it includes compressed content only for inputs of at least 4,000 characters and otherwise adds diagnostics or a summary. Skill-like text with YAML frontmatter is also skipped by the shared paths.

Claude Code requires version 2.1.121 or later for `updatedToolOutput`. On older or unknown versions, response compression is disabled to avoid duplicating the original. Structured tool outputs preserve their host schema and do not switch to textual TOON; JSON carried as a string can use TOON when it is smaller.

## Manage adapters with anolisa (recommended)

### 1. Scan frameworks

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
```

Enable only frameworks that you use. When enabling more than one, run and verify each command separately.

OpenCode is the exception to this section; use its bundled install script under [Manual integration after npm installation](#manual-integration-after-npm-installation).

For OpenClaw, anolisa first attempts a normal install and does not add an unsafe-install bypass by default. If OpenClaw rejects the plugin on its safety scan, read the reported findings. Only after accepting them, retry explicitly:

```bash
anolisa adapter enable tokenless openclaw \
  --allow-unsafe-plugin-install
```

On OpenClaw releases where the underlying bypass is unsupported or a deprecated no-op, anolisa refuses this option; follow the error's `security.installPolicy` guidance instead.

If Tokenless was installed in system mode, use the same scope:

```bash
sudo anolisa adapter enable tokenless <framework>
```

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

For system mode:

```bash
sudo anolisa adapter disable tokenless <framework>
```

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

## Framework activation notes

### cosh

Extensions are discovered at startup. Restart cosh, run a shell-tool task, and inspect `tokenless stats list`.

### OpenClaw

The install script uses OpenClaw's unsafe-install override as described above. Restart the gateway after accepting and installing the plugin. Response compression, Tool Ready, and RTK rewriting default to enabled in the plugin code; TOON defaults to disabled.

### Hermes

The plugin takes effect in a new Hermes session. Restart Hermes and run a shell-tool task.

### Qoder

Qoder IDE and qodercli may cache plugin configuration. Fully restart the IDE after enabling or upgrading. If an old hook path is reported, see [Qoder plugin cache issue](troubleshooting.md#qoder-plugin-cache-issue).

### Claude Code

The marketplace plugin takes effect after restarting Claude Code. The install script may also offer a plugin refresh command.

### Codex

The plugin loads in a new Codex session. Close the old session and start a new one before verifying statistics. Its PostToolUse hook is additive: use statistics as candidate-compression telemetry, not as proof that the original Codex tool output left the prompt.

### OpenCode

OpenCode discovers global local plugins at startup. Use the bundled Tokenless lifecycle script described above, restart OpenCode after installation or removal, then run a tool call and inspect `tokenless stats list`. The script resolves the configuration directory from `TOKENLESS_OPENCODE_CONFIG_DIR`, then `OPENCODE_CONFIG_DIR`, then `XDG_CONFIG_HOME/opencode`, and finally `~/.config/opencode`. Installation creates only `plugins/tokenless.js` as a managed symlink and refuses to replace an unrelated file at that path.

### Qwen Code

The extension loads in a new Qwen Code session. Restart and run one tool call to verify it.

## Verify the actual integration

Do not treat a zero install exit code as the only success criterion. At minimum, run:

```bash
tokenless --version
anolisa adapter status tokenless
tokenless stats list --limit 5
```

Then execute a tool task with visible output in the target agent. If `stats list` remains empty, follow [No statistics appear after enabling the adapter](troubleshooting.md#no-statistics-appear-after-enabling-the-adapter).

## Related documents

- [Quick Start](QUICKSTART.md)
- [Measuring savings](measuring-savings.md)
- [Configuration and data privacy](configuration-and-privacy.md)
- [Troubleshooting](troubleshooting.md)
