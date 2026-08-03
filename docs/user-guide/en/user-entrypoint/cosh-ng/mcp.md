# Connect an MCP Server

[中文版](../../../zh/user-entrypoint/cosh-ng/mcp.md)

An MCP server gives the Agent tools supplied by a local process or remote
service. After this guide, the server will appear in `/mcp list`, its tools
will be visible in `/mcp inspect`, and the Agent can request them during a
task.

## Before you begin

Install and start cosh-ng once. You also need the command or Streamable HTTP
URL supplied by the MCP server owner. The local example below uses `npx`, so
Node.js must be available for that example.

Put MCP definitions in `~/.copilot-shell/config.toml`. Administrators may use
`/etc/copilot-shell/config.toml`. Project configuration cannot add MCP servers
because opening a repository must not start an external program or grant it
credentials.

## Add a local stdio server

Add a server with a `command` and its arguments. Replace the workspace path
with a directory the server is allowed to read.

```toml
[mcp.servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/absolute/path/to/workspace"]
startup_timeout_ms = 30000
timeout_ms = 10000
allowed_tools = ["read_file", "list_directory"]
```

cosh starts this command directly without a Shell. Add required environment
variables explicitly when the server needs them.

```toml
[mcp.servers.filesystem.env]
SERVICE_TOKEN = "${FILESYSTEM_MCP_TOKEN}"
```

## Add a remote HTTP server

Use `url` for a Streamable HTTP endpoint. A server must use either `command`
or `url`.

```toml
[mcp.servers.search]
url = "https://mcp.example.com/mcp"
allowed_tools = ["query"]

[mcp.servers.search.oauth]
scopes = ["search"]
```

For a service that gives you a static token, replace the OAuth section with
`bearer_token = "${SEARCH_MCP_TOKEN}"`. OAuth login requires that
`bearer_token` is absent. Remote endpoints require HTTPS, while loopback HTTP
is accepted for local development.

## Connect and inspect the tools

Start `cosh` in the workspace the server should receive through MCP roots.
Run these commands at the cosh prompt.

```text
/mcp list
/mcp connect filesystem
/mcp inspect filesystem
```

`list` confirms that cosh loaded the definition. `connect` starts or contacts
the server and discovers its tools. `inspect` shows the discovered names and
the names exposed to the Agent. Credentials are not printed.

For an HTTP server that uses OAuth, begin with the slash command below.

```text
/mcp login search
```

cosh displays the command that must run at the Shell prompt for the interactive
browser flow. Run that command, finish authorization, then connect the server.

```text
/mcp connect search
/mcp inspect search
```

## Use an MCP tool

Describe the task and name the connected service when it helps the Agent pick
the right tool.

```text
$ use the filesystem MCP tools to list Markdown files in this workspace; do not modify them
```

The approval card shows the MCP tool and its input before execution when the
current mode requires consent. `allowed_tools` controls which discovered tools
are exposed to the Agent. It does not remove the normal approval boundary.

## Refresh or disconnect a server

Run `refresh` after the server changes its tool list. Disconnect a server when
you no longer want cosh to start or contact it.

```text
/mcp refresh filesystem
/mcp disconnect filesystem
```

`disconnect` disables the server and removes saved OAuth credentials. Use
`/mcp connect <server>` to enable it again. `/mcp logout <server>` removes OAuth
credentials without changing the server definition.

If an Agent task is active, cosh waits for a safe boundary before applying a
connection change. The next task sees the refreshed tool set.

## Troubleshooting

| What you see | What to check |
|---|---|
| `/mcp list` shows no servers | Confirm that the definition is in the system or user configuration, not project configuration |
| A local server does not start | Check the executable path, arguments, explicit environment, and `startup_timeout_ms` |
| The server connects but exposes no tools | Check `allowed_tools`; an empty list exposes none |
| A remote server is rejected | Use HTTPS unless the endpoint is loopback, then check its token or OAuth settings |
| OAuth login cannot start | Remove `bearer_token`, run `/mcp login <server>`, and follow the displayed Shell instruction |

MCP tool output entering Agent context is limited to 64 KiB. External tool
descriptions never lower the current approval policy.
