# cosh-ng Quick Start

[中文版](../../../zh/user-entrypoint/cosh-ng/QUICKSTART.md)

cosh-ng adds an Agent to a normal bash or zsh session. Start with `cosh`, then
run commands or describe a larger task in natural language without leaving the
terminal.

## 1. Install

Install the ANOLISA CLI and cosh-ng:

```bash
curl -fsSL https://get.agentic-os.sh | bash
sudo anolisa --install-mode system install cosh-ng
```

Alibaba Cloud Linux users can alternatively install the RPM:

```bash
sudo yum install cosh-ng
```

These packaged installation paths currently target Linux. On macOS, follow
the [developer setup](../../../../developer-guide/en/cosh-ng/getting-started.md)
to build from source.

Verify the installation:

```bash
cosh --version
cosh-cli --version
```

Package and service mutations require root privileges; workspace checkpoint
commands also require the `ws-ckpt` daemon.

## 2. Enter your AI terminal

Start cosh from the project or system directory you want the Agent to work in:

```bash
cd your-project
cosh
```

Run shell commands exactly as before, or describe a task in natural language:

```text
$ git status
$ find the cause of the last failed deployment; inspect first and do not change anything
```

The Agent streams its work into the terminal. When an operation needs consent,
cosh shows an approval or question card instead of hiding the action in a
background process.

Useful first commands:

```text
/auth                         choose or update provider authentication
/help                         show available slash commands
/status                       show the current runtime and session
/mode approval recommend      confirm every Agent tool call
/session list                 list resumable conversations for this workspace
```

Use `/session list --all` to find conversations created in other workspaces.
To resume one, start cosh in the workspace where that conversation was created.

## 3. Reuse work

Inspect the reusable Skills available to the current workspace:

```text
/skills list
/skills detail service-health
```

Workspace, user, extension, and system Skill directories are combined by
priority. See [Skills](core/skills.md) for the search order and format.

## 4. Continue with your task

| What you want to do | Read next |
|---|---|
| Control approval and safety | [Tool approval](shell/approval.md) |
| Resume or compact conversations | [Session recovery](shell/session-recovery.md) |
| Choose a model and authenticate | [Model providers](core/providers.md) |
| Add tools from another service | [Connect an MCP server](mcp.md) |
| Automate package, service, checkpoint, or audit work | [Manage system operations](cli/overview.md) |
| Integrate another frontend | [Headless mode](core/headless-mode.md) |

The [full user guide](README.md) groups the remaining pages by user task.
