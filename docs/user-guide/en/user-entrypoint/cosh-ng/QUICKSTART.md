# cosh-ng Quick Start

[中文版](../../../zh/user-entrypoint/cosh-ng/QUICKSTART.md)

cosh-ng adds an Agent to a normal bash or zsh session. Start `cosh`, run Shell commands as usual, and describe a larger task in natural language when you need help.

## 1. Install

Install the ANOLISA CLI and cosh-ng:

```bash
curl -fsSL https://get.agentic-os.sh | bash
sudo anolisa --install-mode system install cosh-ng
```

Alibaba Cloud Linux users can install the RPM instead:

```bash
sudo yum install cosh-ng
```

Verify both user-facing commands:

```bash
cosh --version
cosh-cli --version
```

Package and service changes normally need root privileges. Workspace checkpoint commands also need a running `ws-ckpt` daemon.

These packaged paths target Linux. Source builds are for contributors; follow the [developer setup](../../../../developer-guide/en/cosh-ng/getting-started.md) after the packaged options above.

## 2. Start the terminal

Start `cosh` in the project or system directory where the Agent should work:

```bash
cd your-project
cosh
```

Run commands in the same session, and describe a larger task as ordinary input:

```text
$ git status
```

For example, ask the Agent to investigate the last failed deployment and inspect it without making changes.

When an operation needs consent, cosh shows an approval or question card before it proceeds.

Useful first commands:

```text
/auth
/help
/status
/mode approval recommend
/session list
```

`/auth` chooses or updates provider authentication, `/help` lists slash commands, `/status` shows runtime and session status, `/mode approval recommend` asks for confirmation before each Agent tool call, and `/session list` lists resumable conversations in this workspace.

Use `/session list --all` to include conversations from other workspaces. Resume a conversation from the workspace where it was created.

## 3. Reuse Skills

List and inspect Skills available to the current workspace:

```text
/skills list
/skills detail service-health
```

Workspace, user, extension, and system Skill directories are merged by priority. See [Skills](core/skills.md) for the search order and file format.

## 4. Continue with a task

| Goal | Read next |
|---|---|
| Control approval and safety | [Tool approval](shell/approval.md) |
| Resume or compact conversations | [Session recovery](shell/session-recovery.md) |
| Choose a model and authenticate | [Model providers](core/providers.md) |
| Connect tools from another service | [Connect an MCP server](mcp.md) |
| Automate package, service, checkpoint, or audit work | [Structured OS CLI](cli/overview.md) |
| Integrate another frontend | [Headless mode](core/headless-mode.md) |

The [full user guide](README.md) is organized by task.
