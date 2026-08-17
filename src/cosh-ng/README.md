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

The commands below use the `cosh agent` launcher installed by ANOLISA or the
RPM. A source or unified build installs the bare Gateway binary instead; use
`cosh-gateway doctor`, `cosh-gateway run`, or `cosh-gateway task` with the same
remaining arguments.

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

The packaged Gateway provides a contained local Task Plane. It schedules Tasks
only inside the packaged systemd service, which owns the complete Runtime
cgroup after a Gateway hard crash. The `gateway-brokered-v1` Core profile is
intentionally task-only: its runtime inventory contains only the side-effect-free
`ask_user_question` capability. It does not expose checkpoint, write, Shell,
slash-command, Web, or remote capabilities, and this profile has no approvable
side effect.

Configure the workspace and start the account-named Gateway instance:

```bash
sudo install -d -m 0755 /etc/cosh
sudo install -m 0600 /dev/null "/etc/cosh/gateway-$USER.env"
printf '%s\n' \
  "COSH_GATEWAY_WORKSPACE=$PWD" | \
  sudo tee "/etc/cosh/gateway-$USER.env" >/dev/null
sudo systemctl start "cosh-gateway@$USER.service"
gateway_socket="/run/cosh-gateway-$USER/gateway.sock"
printf '%s\n' 'inspect the failed service' | \
  cosh agent task --socket "$gateway_socket" submit \
    --runtime core --runtime-profile gateway-brokered-v1 \
    --idempotency-key '<stable-submit-key>'
cosh agent task --socket "$gateway_socket" get '<tsk_UUID>'
cosh agent task --socket "$gateway_socket" events '<tsk_UUID>' --after 0 --limit 64
printf '%s\n' 'answer to the question' | \
  cosh agent task --socket "$gateway_socket" append '<tsk_UUID>' \
    --input-request-id '<inp_UUID>' --idempotency-key '<stable-input-key>'
cosh agent task --socket "$gateway_socket" cancel '<tsk_UUID>' --run-id '<run_UUID>' \
  --idempotency-key '<stable-cancel-key>'
cosh agent task --socket "$gateway_socket" retry '<tsk_UUID>' \
  --previous-run-id '<run_UUID>' --idempotency-key '<stable-retry-key>'
```

The daemon generates and persists its installation ID on first start; an
operator may provision one explicitly with `--installation-id`. Replace the
typed identifiers with values returned by the Task API. The Task API supports
`submit`, `get`, `events`, `append`, `cancel`, `retry`, and
`resolve-approval`; `append` answers the profile's durable user questions, while
this profile does not generate approval requests.
Direct `serve` fails closed without the packaged unit's live `--systemd-unit`
proof, which is verified before the socket or database is created. The daemon
authenticates the Unix peer as a local OS actor, fixes the target to
`workspace/cosh/task-only-v1`, admits only the `core`/
`gateway-brokered-v1` selector and configured canonical workspace, persists
Runtime bindings, and dispatches durable Outbox work through the scheduler.
Use `doctor` and `run`, not `serve`, for uncontained local ACP interoperability;
those direct ACP commands are not governed by the durable Task Plane.
The Task Plane has no checkpoint or ws-ckpt dependency. The existing
`cosh-cli checkpoint` commands remain a separate system-operations path and do
not add checkpoint capability to this Gateway profile.

`SIGINT` and `SIGTERM` trigger bounded scheduler and Runtime shutdown before the
daemon exits. The daemon remains Unix-only and does not open a remote listener.

The repository includes fake-adapter conformance coverage for the direct ACP
path. Run the separate real Codex/Claude adapter checks and manual Terminal
acceptance before treating a particular ACP installation as production-validated.

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
