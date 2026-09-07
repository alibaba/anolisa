# cosh-ng User Guide

[中文版](../../../zh/user-entrypoint/cosh-ng/README.md)

cosh-ng is an AI-native Linux terminal with Enhanced Assisted as its default
and an explicit hook-free Native integration. Start with the quick start, then
use the task-based links below for the feature or command you need.

## Start here

- [Quick start](QUICKSTART.md) — install cosh-ng and run a first task.
- [Model providers](core/providers.md) — configure authentication and select a provider.
- [Configuration](configuration.md) — review files, settings, and precedence.
- [Supported platforms](supported-distros.md) — check package and service backends.

## Work in the terminal

| Goal | Read next |
|---|---|
| Use Shell commands and natural-language tasks together | [Interactive terminal](shell/overview.md) |
| Choose when Agent tool calls require confirmation | [Tool approval](shell/approval.md) |
| Resume or compact a conversation | [Session recovery](shell/session-recovery.md) |
| Learn slash commands and keyboard behavior | [Interactive behavior](shell/interactive-mode.md) |

## Add capabilities

| Goal | Read next |
|---|---|
| Share instructions across a project or team | [Skills](core/skills.md) |
| Connect tools from a local process or remote service | [Connect an MCP server](mcp.md) |
| Bundle Skills, Hooks, settings, and tools | [Extensions](core/extensions.md) |
| Run checks around Agent lifecycle events | [Hooks](core/hooks.md) |

## Manage system operations

Use read-only commands first. Add `--dry-run` to a supported package or service mutation before making a change; these operations usually need root privileges.

| Goal | Read next |
|---|---|
| Find, install, or remove packages | [Package management](cli/package-management.md) |
| Inspect or change systemd services | [Service management](cli/service-management.md) |
| Use the existing `cosh-cli` workspace checkpoint commands | [Workspace checkpoints](cli/checkpoint.md) |
| Check policy decisions and audit events | [Security audit](cli/audit.md) |

The workspace checkpoint page describes the existing `cosh-cli` system-operations
path. It is separate from the task-only Gateway profile; the packaged Gateway
does not depend on `ws-ckpt` or expose checkpoint operations.

## Integrate and automate

The `cosh agent` launcher is installed by ANOLISA and RPM packages. Source and
unified builds install the bare Gateway binary instead; substitute
`cosh-gateway doctor`, `cosh-gateway run`, or `cosh-gateway task` and keep the
remaining arguments unchanged.

- Run `cosh agent doctor --profile codex --workspace "$PWD"` to verify a
  separately installed `codex-acp`, or select `claude-code` for
  `claude-agent-acp`. Run one turn by piping a bounded UTF-8 prompt into
  `cosh agent run`; add `--output jsonl` for stable streamed events. COSH does
  not run `npx`, download packages, or accept arbitrary adapter commands.
  Permission requests use `/dev/tty`, leaving stdin dedicated to the prompt.
  The default `--permission prompt` offers only `allow_once` and `reject_once`;
  no TTY, unsupported choices, EOF, and `--permission deny` all cancel without
  authorization. Redacted append-only evidence defaults to
  `$XDG_STATE_HOME/cosh/gateway/permission-evidence.jsonl`, falling back to
  `$HOME/.local/state/cosh/gateway/permission-evidence.jsonl`. Use an absolute
  `--permission-evidence PATH` to override it. COSH stores hashes and the
  decision class, never raw prompts, tool arguments, option labels, session
  identifiers, or workspace paths. Evidence persistence failure cancels the
  callback and fails the run. These direct ACP commands are ungoverned by the
  durable Gateway Task Plane and are intended for local interoperability.
- For durable local Tasks, use the packaged system-scope
  `cosh-gateway@.service`. It selects the contained `core` runtime with the
  `gateway-brokered-v1` profile and admits the configured canonical workspace:

  The unit defaults Core `HOME` to
  `/var/lib/cosh-gateway-%i/core-home`, below its private systemd
  `StateDirectory`. Store the provider configuration at
  `/var/lib/cosh-gateway-$USER/core-home/.copilot-shell/config.toml`, or use
  `/etc/copilot-shell/config.toml` as a system configuration. Do not set
  `HOME` to a path outside that `StateDirectory` in
  `/etc/cosh/gateway-$USER.env`. `EnvironmentFile` values override the unit's
  safe default, while the admitted workspace and other host paths are
  read-only for this contained Core profile.

  ```bash
  sudo install -d -m 0755 /etc/cosh
  sudo install -m 0600 /dev/null "/etc/cosh/gateway-$USER.env"
  printf '%s\n' \
    "COSH_GATEWAY_WORKSPACE=$PWD" | \
    sudo tee "/etc/cosh/gateway-$USER.env" >/dev/null
  sudo systemctl start "cosh-gateway@$USER.service"
  gateway_socket="/run/cosh-gateway-$USER/gateway.sock"
  ```

  The unit passes `--systemd-unit`; Gateway verifies live cgroup membership,
  control-group kill, final `SIGKILL`, main-process exit tracking, and disabled
  delegation before it binds a socket. Direct `serve` fails closed without that
  proof. The service also hides the per-user service-manager socket from Runtime
  descendants. Startup canonicalizes the workspace and fixes the admitted target
  to `workspace/cosh/task-only-v1` and the Runtime selector to
  `core`/`gateway-brokered-v1`. The daemon authenticates each Unix peer as a
  local OS actor; a submission with a different target or selector is rejected
  before Task creation.
- From another Terminal, set `gateway_socket` to the same absolute path and pipe
  the intent into the Task API:

  ```bash
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

  The Task API supports `submit`, `get`, `events`, `append`, `cancel`, `retry`,
  and `resolve-approval`. `append` answers the profile's durable
  `ask_user_question` request. `resolve-approval` remains part of the generic
  API, but this profile has no approvable side effect and therefore produces no
  approval flow. Idempotency keys make retries safe after uncertain client I/O;
  durable Task, Runtime, and Outbox state supports inspection, cancellation, and
  explicit retry without replaying an unknown side effect.
- The task-only profile intentionally exposes no checkpoint, write, Shell,
  slash-command, Web, channel, or remote capability. Interactive slash commands
  remain owned by `cosh-shell`; they are not Gateway Task commands. `SIGINT` and
  `SIGTERM` initiate bounded scheduler and Runtime shutdown, and the Gateway
  listens only on its local Unix socket. Repository Fake-Adapter coverage is
  automated; real Codex/Claude Adapter checks and manual Terminal acceptance
  remain separate installation-specific gates.
- [Structured OS CLI](cli/overview.md) — command domains and safe automation patterns.
- [Output format](output-format.md) — the `CoshResponse<T>` success and error envelope.
- [Headless mode](core/headless-mode.md) — JSONL integration for other frontends.
- [Agent tools](core/tools.md) — tool boundaries and approval behavior.
