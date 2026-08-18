# cosh-ng Quick Start

[中文版](../../../zh/user-entrypoint/cosh-ng/QUICKSTART.md)

cosh-ng adds an Agent to a normal bash or zsh session. Start `cosh`, run Shell commands as usual, and describe a larger task in natural language when you need help.

## 1. Install

On Alibaba Cloud Linux 4, install the ANOLISA CLI, then install cosh-ng from
the RPM backend in system scope:

```bash
curl -fsSL https://get.agentic-os.sh | bash
export PATH="$HOME/.local/bin:$PATH"
sudo "$HOME/.local/bin/anolisa" --install-mode system install cosh-ng --backend rpm
```

The public installer can combine the CLI and component installation. It prompts
for `sudo` only when running the component action:

```bash
curl -fsSL https://get.agentic-os.sh | bash -s -- --cosh-ng --backend rpm --install-mode system
export PATH="$HOME/.local/bin:$PATH"
```

On macOS arm64, use user scope instead:

```bash
curl -fsSL https://get.agentic-os.sh | bash -s -- --cosh-ng --backend raw --install-mode user
export PATH="$HOME/.local/bin:$PATH"
```

Alibaba Cloud Linux 4 users can also install the RPM directly:

```bash
sudo yum install cosh-ng
```

Verify both user-facing commands:

```bash
cosh --version
cosh-cli --version
```

Package and service changes normally need root privileges. Workspace checkpoint commands also need a running `ws-ckpt` daemon.

The published Linux raw contract is not currently portable across all routed
distributions, so it is not the recommended Linux installation path. The raw
package supports macOS arm64, where Linux-only package and service operations
remain unavailable. Source builds are for contributors; follow the
[developer setup](../../../../developer-guide/en/cosh-ng/getting-started.md)
after the packaged options above.

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
