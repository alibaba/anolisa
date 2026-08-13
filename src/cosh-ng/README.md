# cosh-ng

[中文版](README_zh.md)

cosh-ng is an AI-native terminal built around the shell you already use.
Start `cosh` to run bash or zsh as usual, then describe larger tasks in natural
language when you want the Agent to investigate or act. Shell commands, Skills,
approval cards, and resumable conversations stay in one terminal. Structured
JSON and JSONL interfaces are available for automation and Agent integration.

## Why cosh-ng

| In a conventional terminal | In cosh-ng |
|---|---|
| You translate intent into commands | Ask in natural language or run commands directly |
| Automation is scattered across scripts | Package repeatable workflows as Skills |
| AI context is tied to one chat window | Resume workspace-scoped Agent conversations |
| AI actions are hard to inspect | Review tool calls in approval cards and audit records |
| Every distro has different system commands | Use `cosh-cli` for stable, structured OS operations |

Interactive programs, pipes, redirects, job control, bash/zsh configuration,
and `Ctrl+C` continue to work in the foreground terminal.

## Install

On Alibaba Cloud Linux 4, install cosh-ng from the RPM backend in system scope
with the ANOLISA CLI:

```bash
curl -fsSL https://get.agentic-os.sh | bash
export PATH="$HOME/.local/bin:$PATH"
sudo "$HOME/.local/bin/anolisa" --install-mode system install cosh-ng --backend rpm
```

The public installer can combine those steps:

```bash
curl -fsSL https://get.agentic-os.sh | bash -s -- --cosh-ng --backend rpm --install-mode system
export PATH="$HOME/.local/bin:$PATH"
```

Use the same entry point for later updates or removal:

```bash
curl -fsSL https://get.agentic-os.sh | bash -s -- --cosh-ng --install-mode system --upgrade
curl -fsSL https://get.agentic-os.sh | bash -s -- --cosh-ng --install-mode system --uninstall
```

On macOS arm64, use user scope instead:

```bash
curl -fsSL https://get.agentic-os.sh | bash -s -- --cosh-ng --backend raw --install-mode user
export PATH="$HOME/.local/bin:$PATH"
```

On Alibaba Cloud Linux 4, the RPM is also available directly:

```bash
sudo yum install cosh-ng
```

The published Linux raw contract is not currently portable across all routed
distributions, so it is not the recommended Linux installation path. The raw
package supports macOS arm64, where Linux-only package and service operations
remain unavailable. Source builds are for contributors; follow the
[developer setup](../../docs/developer-guide/en/cosh-ng/getting-started.md).

## Start in 30 seconds

```bash
cd your-project
cosh
```

Then mix shell commands and Agent requests in the same session:

```text
$ git status
$ explain why this service keeps restarting and show me the evidence
$ /agent
$ /skills list
$ /session status
```

Use `/auth` to choose a supported provider plan, `/help` to list current slash
commands, and `/mode approval recommend` when every Agent tool call should wait
for confirmation. Approval settings use `recommend`, `auto`, or `trust` across
the shell and Core. With the cosh-core runtime, `/agent` opens a one-shot
Composer that accepts a leading `/skill:<name>` and validated workspace-local
`@path` references.

To run one locally installed ACP adapter without entering the interactive
Shell, verify it first and then pipe the prompt through stdin:

```bash
cosh agent doctor --profile codex --workspace "$PWD"
printf '%s\n' 'summarize the current changes' | \
  cosh agent run --profile codex --workspace "$PWD"
```

The first release accepts only the built-in `codex` and `claude-code`
profiles. Install the corresponding `codex-acp` or `claude-agent-acp`
executable separately; COSH never invokes `npx` or downloads an adapter at
runtime. A permission callback prompts only on the local controlling terminal;
without one, or with `--permission deny`, COSH cancels it. Once-only decisions
are recorded as redacted evidence under the private local state directory.

## Documentation

- [User guide](../../docs/user-guide/en/user-entrypoint/cosh-ng/README.md)
- [Connect an MCP server](../../docs/user-guide/en/user-entrypoint/cosh-ng/mcp.md)
- [Interactive terminal](../../docs/user-guide/en/user-entrypoint/cosh-ng/shell/overview.md)
- [Configuration](../../docs/user-guide/en/user-entrypoint/cosh-ng/configuration.md)
- [Manage system operations](../../docs/user-guide/en/user-entrypoint/cosh-ng/cli/overview.md)
- [Headless integration](../../docs/user-guide/en/user-entrypoint/cosh-ng/core/headless-mode.md)
- [Developer getting started](../../docs/developer-guide/en/cosh-ng/getting-started.md)
- [Architecture](../../docs/developer-guide/en/cosh-ng/architecture.md)
- [Contributing](CONTRIBUTING.md)

## Contribute

Source builds are a contributor workflow. Start with the
[developer guide](../../docs/developer-guide/en/cosh-ng/getting-started.md).

## License

Apache-2.0
