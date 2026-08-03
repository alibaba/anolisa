# cosh-ng

[中文版](README_zh.md)

cosh-ng is an AI-native Linux terminal built around the shell you already use.
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

Install with the ANOLISA CLI:

```bash
curl -fsSL https://get.agentic-os.sh | bash
sudo anolisa --install-mode system install cosh-ng
```

On Alibaba Cloud Linux, the RPM is an alternative:

```bash
sudo yum install cosh-ng
```

These packaged installation paths currently target Linux. On macOS, follow
the [developer setup](../../docs/developer-guide/en/cosh-ng/getting-started.md)
to build from source.

## Start in 30 seconds

```bash
cd your-project
cosh
```

Then mix shell commands and Agent requests in the same session:

```text
$ git status
$ explain why this service keeps restarting and show me the evidence
$ /skills list
$ /session status
```

Use `/auth` to choose a supported provider plan, `/help` to list current slash
commands, and `/mode approval recommend` when every Agent tool call should wait
for confirmation.

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
