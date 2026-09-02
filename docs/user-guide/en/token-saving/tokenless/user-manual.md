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

This path produces only the standalone `tokenless` CLI. It does not install `rtk` or the agent integration resources. To use the complete feature set in an agent, install through the anolisa CLI as described in the [Quick Start](QUICKSTART.md).

## Build the Python SDK from source

CPython applications can use Tokenless in process instead of starting the CLI for every lifecycle
operation:

```bash
make python-wheel
python3 -m venv /tmp/tokenless-python
/tmp/tokenless-python/bin/pip install target/wheels/anolisa_tokenless-*.whl
```

The build requires a discoverable CPython 3.11+ development environment. The wheel uses the
CPython 3.11 stable ABI and remains specific to the operating system and architecture for which it
was built.

The Python SDK has two layers. The `anolisa-tokenless` package exposes the framework-neutral
`TokenlessSdk`, direct `TokenlessRuntime` operations, and typed `TokenlessStats` queries. The
same-version `anolisa-tokenless-agentscope` package maps that generic lifecycle to AgentScope. See
the [Python SDK guide](sdk.md) for both layers, runnable examples, and configuration.

## Capabilities and boundaries

| Capability | Behavior implemented in the current code | Important boundary |
|------------|------------------------------------------|--------------------|
| Schema compression | Removes `title` and `examples`, removes fenced and inline code from descriptions, collapses whitespace, and truncates descriptions | Common BeforeModel passes lossy transformations through without marker-authorized recovery; OpenCode's per-tool path and the direct CLI still compress (Qwen Code skips the declared event) |
| Content-aware response compression | Protocol v2 routes successful PostTool JSON to `JsonCompressor`, then accepts only a smaller end-to-end result | Non-JSON domains currently pass through; recoverable truncation requires Agent-facing recovery that verifies the current Marker set |
| TOON encoding | Encodes JSON and keeps the JSON input when the estimated token count does not decrease | Replaces the original when the host accepts text replacement; hosts without replacement capability pass through |
| Command rewriting | Calls `rtk rewrite` and submits the rewritten shell input when a rule is available | The command actually sent to the shell changes; unsupported or denied rewrites pass through |
| Tool Ready | Legacy pre-call checks for declared binaries, versions, configuration, permissions, and optional dependencies | Hard-disabled; it cannot inspect, repair, or block tool execution |
| Stash | Stores content removed by string, array, depth, or schema-description truncation | One-hour TTL and 10,000 live entries by default; other removed fields are not stashed |

The implementation contains no fixed saving-rate guarantee. Results depend on the payload, adapter delivery semantics, and the share of the model context that came from tool data. Measure your own workload as described in [Measuring savings](measuring-savings.md).

## How Tokenless participates in a tool call

After an adapter is enabled, a tool call may pass through these stages:

```text
Before the tool: hard-disabled Tool Ready hook → command rewrite
Before the tool: RTK rewrite → carry output-optimization state
After the tool: status and optimization bypass → JSON-only PostTool Pipeline → optional Stash/TOON → statistics
Before the model: schema compression → visible Marker extraction → conditional Retrieve declaration
Retrieve: visible-Marker authorization → byte-identical Stash read
```

This is a capability map, not a pipeline that every framework runs. For example, the content-aware
protocol path currently serves Cosh-NG, OpenClaw, Hermes, Qoder, supported Claude Code releases,
OpenCode, and DeepSeek Harness. Codex and Qwen Code do not replace post-tool output under their
current host contracts. See
[Agent integration](framework-integration.md).

## Behaviors to understand

### Installation does not enable every adapter

`anolisa install tokenless` installs the component and its adapter resources. To make an agent use Tokenless automatically, also run:

```bash
anolisa adapter enable tokenless <framework>
```

CLI-only use does not require an adapter.

### “Compression off” affects only compression operations

With `compression_enabled=false` or `TOKENLESS_COMPRESSION_ENABLED=0`, `compress`,
`compress-schema`, `compress-response`, and `compress-toon` still calculate predicted savings and
may write statistics, but return the original input. They do not write Stash entries in this mode.

This setting does not disable RTK command rewriting, adapter execution, or retrieval. Tool Ready is independently hard-disabled. To stop all Tokenless behavior in an agent, disable the adapter:

```bash
anolisa adapter disable tokenless <framework>
```

### Reversible compression is conditional

Active response and schema truncation stash the removed payload in
`~/.tokenless/stash.db` by default and add a marker such as:

```text
<<tokenless:0123456789abcdef01234567>>
```

The payload can be recovered locally through the trusted `tokenless retrieve` command. Protocol v2
agent-facing retrieval first requires the requested Marker to be present in the model's current
`visible_markers` set. The old stateless MCP server was removed because it had no trustworthy
model-visibility context. Recovery is unavailable when:

- `--no-stash` was used.
- Compression was running in dry-run mode.
- The Stash database was unavailable or a write failed.
- The entry exceeded its TTL.
- The 10,000-live-entry capacity evicted an older entry.
- The caller uses a different Stash database path.

Stash does not make all compression reversible. Removed `debug`/`trace` fields, `null` and empty values, schema `title`/`examples`, and Markdown formatting are not stored for retrieval. Validate critical payloads with representative data before enabling active compression.

### Processing errors usually fail open

Compression and rewrite hooks normally return no modification when `tokenless` or `rtk` is missing
or compression provides no savings. For Protocol v2 `compress`, normal non-application outcomes
return a result with exit code `0`; malformed transport exits `2`, while RTK timeout, unauthorized
Retrieve, Stash failure, and Pipeline failure exit `1` without response JSON. Tool Ready is
hard-disabled before its legacy check, repair, and blocking logic. Post-tool failure attribution is
independent and remains unchanged.

Command rewriting also changes the shell command submitted by the host. Most adapters replace the command input directly; Hermes blocks the first call and tells the agent to retry with the rewritten command. Validate important command workflows as well as compressed output.

## Supported Agent adapters

| Agent product | Integration | Current code path |
|-----------|-------------|-------------------|
| cosh | Extension | Hard-disabled Tool Ready, rewrite, Schema; Cosh-NG replaces eligible pipeline output, while legacy Copilot Shell passes post-tool output through |
| OpenClaw | Plugin | Hard-disabled Tool Ready, `exec` rewrite, persisted-result replacement, optional TOON; no Schema |
| Hermes | Plugin | Hard-disabled Tool Ready, Core-owned block-and-retry rewrite, lossless result replacement with Core-selected TOON; no Schema/Retrieve |
| Qoder | Plugin | Hard-disabled Tool Ready, rewrite, response pipeline through `updatedToolOutput`; no Schema |
| Claude Code | Marketplace plugin | Hard-disabled Tool Ready, Bash rewrite, response replacement on Claude Code 2.1.121 or later; conditional TOON; no Schema |
| Codex | Plugin | Hard-disabled Tool Ready, RTK rewrite, environment-failure diagnostics; no response/TOON replacement or Schema |
| OpenCode | Plugin | Hard-disabled Tool Ready, Bash rewrite, tool-output replacement with response + TOON, Schema |
| Qwen Code | Extension | Hard-disabled Tool Ready, rewrite; current host lacks post-tool replacement and skips the declared BeforeModel event |

## Supported Agent development frameworks

| Framework | Integration | Current code path |
|-----------|-------------|-------------------|
| AgentScope | In-process Python middleware | Replaces successful final tool responses and exposes a marker-scoped retrieval Tool through a separate Python package |

## Find documentation by task

| I want to | Document |
|-----------|----------|
| Install and verify for the first time | [Quick Start](QUICKSTART.md) |
| Build the standalone CLI from source | [This page · Build the standalone CLI from source](#build-the-standalone-cli-from-source) |
| Use the in-process Python SDK | [Python SDK](sdk.md) |
| Integrate AgentScope | [AgentScope SDK integration](sdk/agentscope.md) |
| Connect an Agent product | [Agent integration](framework-integration.md) |
| Compress or retrieve manually | [CLI reference](cli-reference.md) |
| Inspect savings or content changes, or run a dual comparison | [Measuring savings](measuring-savings.md) |
| Change settings or understand local data | [Configuration and data privacy](configuration-and-privacy.md) |
| Fix missing statistics, adapter, or Stash issues | [Troubleshooting](troubleshooting.md) |
| Diagnose missing schema-compression records | [Troubleshooting · Schema compression produces no statistics](troubleshooting.md#schema-compression-produces-no-statistics) |
| Upgrade or uninstall | [Troubleshooting · Upgrade and uninstall](troubleshooting.md#upgrade-and-uninstall) |

## Recommended rollout

1. Complete the [Quick Start](QUICKSTART.md) with non-sensitive test data.
2. Record a dry-run baseline for the same task.
3. Enable active compression and compare both output quality and savings.
4. Confirm that local-data and SLS behavior meets your requirements.
5. Enable the adapter for production agents.

The `tokenless --help` output from the installed version is the final authority for CLI and configuration behavior.
