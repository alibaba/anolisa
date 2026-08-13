# Tokenless User Manual

[中文版](../../../zh/token-saving/tokenless/user-manual.md)

Tokenless is designed for tool-heavy AI agents. Its CLI compacts schemas and JSON responses, while its adapters can also rewrite shell commands, check tool dependencies, and pass compressed results to an agent. The exact effect depends on the host framework: some adapters replace the original result, while others add compressed context without removing the original.

Start with the [Quick Start](QUICKSTART.md) if this is your first use.

## Build the standalone CLI from source

Source builds are intended for development and debugging. The project currently validates and supports source builds on Linux only:

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa/src/tokenless
cargo build --release --locked -p tokenless-cli
./target/release/tokenless --version
```

This path produces only the standalone `tokenless` CLI. It does not install `rtk`, `toon`, or the agent integration resources. To use the complete feature set in an agent, install through the anolisa CLI as described in the [Quick Start](QUICKSTART.md).

### Binary tarball (no build toolchain required)

Pre-built release tarballs include an embedded `install.sh` that deploys to any `PREFIX` without requiring Rust, Cargo, or Node.js on the target machine. Linux only (x86_64 / aarch64).

```bash
# Build the tarball (on a Linux build machine)
cd anolisa/src/tokenless
make release
# => dist/tokenless-<VERSION>-<ARCH>.tar.gz

# Deploy on the target machine
tar xzf tokenless-<VERSION>-<ARCH>.tar.gz
cd tokenless-<VERSION>
sudo ./install.sh              # installs to /usr/local (default)
sudo PREFIX=/usr ./install.sh  # installs to /usr
```

The installer deploys:
- `bin/tokenless` and `libexec/anolisa/tokenless/{rtk,toon}` with symlinks in `bin/`
- Adapter resources under `share/anolisa/adapters/tokenless/`
- Component contract at `share/anolisa/components/tokenless/component.toml`
- Cosh extension at `share/anolisa/extensions/tokenless/` (hooks, commands, cosh-extension.json)

**Note:** `PREFIX` must be an absolute path. The installer is an overwrite-style deployment — it replaces the entire adapter resources directory and cosh extension on each run. For non-destructive or staged installs, use `make install` from a source build.

If cosh does not scan `$PREFIX/share/anolisa/extensions` by default (the system scan path is `/usr/share/anolisa/extensions`), you may need to configure cosh to include the extension path for non-standard prefixes.

## Capabilities and boundaries

| Capability | Behavior implemented in the current code | Important boundary |
|------------|------------------------------------------|--------------------|
| Schema compression | Removes `title` and `examples`, removes fenced and inline code from descriptions, collapses whitespace, and truncates descriptions | Only available through the cosh and Qwen Code schema hooks; other users can call the CLI |
| Response compression | Removes exact, case-sensitive debug-field names, `null`, empty strings/arrays/objects, and truncates values past configured limits | Accepts JSON; content-retrieval tools are intentionally skipped by adapters |
| TOON encoding | Encodes JSON and keeps the JSON input when the estimated token count does not decrease | Whether TOON replaces or accompanies the original depends on the adapter |
| Command rewriting | Calls `rtk rewrite` and submits the rewritten shell input when a rule is available | The command actually sent to the shell changes; unsupported or denied rewrites pass through |
| Tool Ready | Checks declared binaries, versions, configuration, permissions, and optional dependencies | `--fix` installs only missing required dependencies and may change the environment |
| Stash | Stores content removed by string, array, depth, or schema-description truncation | One-hour TTL and 10,000 live entries by default; other removed fields are not stashed |

The implementation contains no fixed saving-rate guarantee. Results depend on the payload, adapter delivery semantics, and the share of the model context that came from tool data. Measure your own workload as described in [Measuring savings](measuring-savings.md).

## How Tokenless participates in a tool call

After an adapter is enabled, a tool call may pass through these stages:

```text
Before the tool: Tool Ready check → command rewrite
After the tool: response compression → optional Stash → TOON encoding → statistics
Before the model: schema compression
```

This is a capability map, not a pipeline that every framework runs. For example, OpenClaw disables TOON by default, Codex adds compressed context instead of replacing the original tool result, and only cosh and Qwen Code register schema compression. See [Framework integration](framework-integration.md).

## Behaviors to understand

### Installation does not enable every adapter

`anolisa install tokenless` installs the component and its adapter resources. To make an agent use Tokenless automatically, also run:

```bash
anolisa adapter enable tokenless <framework>
```

CLI-only use does not require an adapter.

### “Compression off” affects only compression operations

With `compression_enabled=false` or `TOKENLESS_COMPRESSION_ENABLED=0`, `compress-schema`, `compress-response`, and `compress-toon`—whether called directly or through an adapter—still calculate predicted savings and may write statistics, but return the original input. They do not write Stash entries in this mode.

This setting does not disable RTK command rewriting, Tool Ready checks, adapter execution, or retrieval. To stop all Tokenless behavior in an agent, disable the adapter:

```bash
anolisa adapter disable tokenless <framework>
```

### Reversible compression is conditional

Active response and schema truncation stash the removed payload in `~/.tokenless/stash.db` by default and add a marker such as:

```text
<<tokenless:0123456789abcdef01234567>>
```

The payload can be recovered through `tokenless retrieve` or the MCP `tokenless_retrieve` tool. Recovery is unavailable when:

- `--no-stash` was used.
- Compression was running in dry-run mode.
- The Stash database was unavailable or a write failed.
- The entry exceeded its TTL.
- The 10,000-live-entry capacity evicted an older entry.
- The caller uses a different Stash database path.

Stash does not make all compression reversible. Removed `debug`/`trace` fields, `null` and empty values, schema `title`/`examples`, and Markdown formatting are not stored for retrieval. Validate critical payloads with representative data before enabling active compression.

### Processing errors usually fail open

Compression and rewrite hooks normally return no modification when `tokenless` or `rtk` is missing, compression provides no savings, or an ordinary processing error occurs. Tool Ready is different: some adapters intentionally block a tool that is still `NOT_READY` after an auto-fix attempt. A Stash write failure may still allow lossy compression to continue.

Command rewriting also changes the shell command submitted by the host. Most adapters replace the command input directly; Hermes blocks the first call and tells the agent to retry with the rewritten command. Validate important command workflows as well as compressed output.

## Supported agent frameworks

| Framework | Integration | Current code path |
|-----------|-------------|-------------------|
| cosh | Extension | Tool Ready, rewrite, response + TOON, Schema; Cosh-NG has a replacement path, while legacy Copilot Shell appends additional context |
| OpenClaw | Plugin | Tool Ready, `exec` rewrite, persisted-result replacement, optional TOON; no Schema |
| Hermes | Plugin | Tool Ready, block-and-retry rewrite, result replacement with response + TOON; no Schema |
| Qoder | Plugin | Tool Ready, rewrite, response + TOON through `additionalContext`; no Schema |
| Claude Code | Marketplace plugin | Tool Ready, Bash rewrite, response replacement on Claude Code 2.1.121 or later; conditional TOON; no Schema |
| Codex | Plugin | Tool Ready, rewrite, response/TOON analysis added as context; the original result is retained; no Schema |
| Qwen Code | Extension | Tool Ready, rewrite, response + TOON through `additionalContext`, Schema |

## Find documentation by task

| I want to | Document |
|-----------|----------|
| Install and verify for the first time | [Quick Start](QUICKSTART.md) |
| Build the standalone CLI from source | [This page · Build the standalone CLI from source](#build-the-standalone-cli-from-source) |
| Connect or switch an agent framework | [Framework integration](framework-integration.md) |
| Compress, retrieve, or run MCP manually | [CLI reference](cli-reference.md) |
| Inspect savings or content changes, or run a dual comparison | [Measuring savings](measuring-savings.md) |
| Change settings or understand local data | [Configuration and data privacy](configuration-and-privacy.md) |
| Fix missing statistics, adapter, or Stash issues | [Troubleshooting](troubleshooting.md) |
| Upgrade or uninstall | [Troubleshooting · Upgrade and uninstall](troubleshooting.md#upgrade-and-uninstall) |

## Recommended rollout

1. Complete the [Quick Start](QUICKSTART.md) with non-sensitive test data.
2. Record a dry-run baseline for the same task.
3. Enable active compression and compare both output quality and savings.
4. Confirm that local-data and SLS behavior meets your requirements.
5. Enable the adapter for production agents.

The `tokenless --help` output from the installed version is the final authority for CLI and configuration behavior.
