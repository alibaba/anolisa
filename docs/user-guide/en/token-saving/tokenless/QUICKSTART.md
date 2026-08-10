# Tokenless Quick Start

[中文版](../../../zh/token-saving/tokenless/QUICKSTART.md)

## 1. What Tokenless does

Tokenless helps an AI agent complete the same work with fewer tokens.

After you turn it on, you do not need to change your prompts or the way you use the agent. Tokenless works automatically in the background.

What you may notice:

- Less token usage on lengthy intermediate results.
- More room for useful information during long tasks.
- Cleaner information for the agent to use when deciding what to do next.

Savings vary by task. Short tasks or tasks that are mostly conversation may show little change, so check the result with your own workload in [View the result](#4-view-the-result).

## 2. Install Tokenless

Install the anolisa CLI first, then use it to install Tokenless:

```bash
curl -fsSL https://get.agentic-os.sh | bash
export PATH="$HOME/.local/bin:$PATH"
anolisa --version
anolisa install tokenless
tokenless --version
```

If `anolisa --version` succeeds, start with `anolisa install tokenless`. The
PATH update above makes a fresh default installation available in the current
shell; new login shells may already include `~/.local/bin`.

## 3. Start using Tokenless

### 3.1 Use Tokenless in your agent

Tokenless can work with:

| Agent | Value used in commands |
|-------|------------------------|
| cosh / Copilot Shell | `cosh` |
| OpenClaw | `openclaw` |
| Hermes | `hermes` |
| Qoder | `qoder` |
| Claude Code | `claude-code` |
| Codex | `codex` |
| Qwen Code | `qwencode` |

Find your agent and turn on Tokenless:

```bash
anolisa adapter scan
anolisa adapter enable tokenless <agent>
anolisa adapter status tokenless
```

Restart the agent CLI, IDE, or gateway after Tokenless is enabled.

#### 3.1.1 Example: OpenClaw

Turn on Tokenless and restart the OpenClaw gateway:

```bash
anolisa adapter enable tokenless openclaw
anolisa adapter status tokenless
```

Then ask OpenClaw to perform a normal task:

> Run the full test suite for this repository and summarize only the failures.

You do not need to mention Tokenless in the prompt.

If OpenClaw rejects the installation during its security check, follow [the OpenClaw instructions](framework-integration.md#openclaw) before retrying.

### 3.2 Use the standalone CLI

You can try response compression directly:

```bash
printf '%s\n' \
  '{"status":"ok","data":{"name":"demo","items":[1,2,3]},"debug":{"trace":"verbose"},"metadata":null}' \
  | tokenless compress-response
```

The command returns valid JSON with removable fields such as `debug` and `metadata` omitted.
If the output is unchanged, the input has no compressible content; retry with JSON that contains `debug`, `null`, or a long string.

## 4. View the result

After using a Shell, API, or other supported tool in your agent, run:

```bash
tokenless stats list --limit 5
tokenless stats summary
```

- `stats list` shows recent results that Tokenless made shorter. Copy a record ID from this list when you want to inspect one result.
- `stats summary` shows the estimated tokens before and after Tokenless processing and the total saved.

For the OpenClaw example above, look for a record containing `openclaw` and confirm that its token count decreases from left to right.

To see what changed in one record:

```bash
tokenless stats diff <record-id>
```

If no record appears, the content may not have passed through Tokenless or may not have become shorter. See [No statistics appear after setup](troubleshooting.md#no-statistics-appear-after-enabling-the-adapter).

Token counts are estimates for content processed by Tokenless, not a direct measurement of the model bill. Statistics and diffs may contain original tool content; avoid sharing their output when it contains sensitive data. See [Measuring savings](measuring-savings.md) and [Configuration and data privacy](configuration-and-privacy.md) for details.

## 5. Platform support

| Platform | anolisa CLI installation |
|----------|--------------------------|
| Linux x86_64/aarch64 | Supported |
| macOS Apple Silicon | Supported |
| macOS x86_64 | Not currently supported |
| Windows or Linux with musl, such as Alpine | Not currently supported |

This page covers installation with the anolisa CLI only. To build the standalone CLI from source, see [User manual · Build the standalone CLI from source](user-manual.md#build-the-standalone-cli-from-source).

## 6. Next steps

- [User manual](user-manual.md): behavior boundaries and documentation map
- [Framework integration](framework-integration.md): enable, verify, and disable each agent
- [CLI reference](cli-reference.md): all subcommands and options
- [Measuring savings](measuring-savings.md): statistics, dual runs, and AgentSight/SLS
- [Configuration and data privacy](configuration-and-privacy.md): toggles, storage, and sensitive data
- [Troubleshooting](troubleshooting.md): common errors, upgrades, and uninstall
