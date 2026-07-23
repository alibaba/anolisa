# Configuration

The three cosh-ng binaries share the configuration file `~/.copilot-shell/config.toml`. Environment variable overrides and CLI parameter precedence are supported.

## Configuration File Locations

Configuration is loaded in the following priority order (highest to lowest):

1. `.copilot-shell/config.toml` (project-level, current directory)
2. `~/.copilot-shell/config.toml` (user-level)
3. `/etc/copilot-shell/config.toml` (system-level)

## cosh-core Configuration

```toml
[ai]
# Active model identifier
active_model = "qwen-plus"
# Output language (optional)
output_language = "zh"

[ai.providers.aliyun]
type = "aliyun"
access_key_id = ""        # Or via ALIBABA_CLOUD_ACCESS_KEY_ID
access_key_secret = ""    # Or via ALIBABA_CLOUD_ACCESS_KEY_SECRET
model = "qwen-plus"

[ai.providers.dashscope]
type = "dashscope"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key = ""              # Or via DASHSCOPE_API_KEY
model = "qwen-plus"

[agent]
# Approval mode: trust | auto | balanced | suggest | strict
approval_mode = "balanced"
# Maximum conversation turns
max_turns = 20

[hooks]
enabled = true

[skills]
# Custom skill search paths
custom_paths = []

[mcp.servers.filesystem]
# Local stdio server.
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
# Startup/discovery timeout; first npx launch may download its package.
startup_timeout_ms = 30000
# Timeout for a subsequent tools/call request.
timeout_ms = 10000
# Omit to expose all discovered tools. Use [] to expose none.
allowed_tools = ["read_file", "list_directory"]

[mcp.servers.remote-search]
# Streamable HTTP endpoint. Do not combine `url` and `command`.
url = "https://mcp.example.com/mcp"
# For static-token authentication instead of OAuth, uncomment:
# bearer_token = "${REMOTE_SEARCH_TOKEN}"
allowed_tools = ["search"]

# OAuth settings are optional; discovery and dynamic client registration are used by default.
[mcp.servers.remote-search.oauth]
scopes = ["search"]

[session]
# Root for workspace-scoped provider conversations
persist_dir = "~/.copilot-shell/cosh-core/sessions"
# Disable to keep turns in memory only; emitted IDs will not be resumed
auto_persist = true

[logging]
level = "warn"
```

The project layer is loaded from
`<workspace>/.copilot-shell/config.toml`, where `workspace` is the path passed
through `--workspace` or the session-management request. Relative
`session.persist_dir` values are resolved from that workspace, not from the
Core process's launcher directory.

## MCP Servers

`cosh-core --headless` can start configured stdio MCP servers or connect to
configured Streamable HTTP MCP endpoints, call
`tools/list`, and register each permitted tool as `mcp__<server>__<tool>`.
The client supports `initialize`, `tools/list`, and `tools/call`. HTTP servers
may reply with JSON or SSE. Streamable HTTP servers can use OAuth with
`cosh-core mcp login <server>`; credentials are stored separately from the
configuration. Deprecated `2024-11-05` HTTP+SSE servers are also supported
through automatic fallback. Hosting cosh-core as an MCP server is not supported.

MCP server definitions are read only from `/etc/copilot-shell/config.toml` and
`~/.copilot-shell/config.toml`. Project-level `.copilot-shell/config.toml` is
ignored for MCP to prevent a checked-out project from starting arbitrary local
programs or connecting to untrusted endpoints. Each server must set exactly
one of `command` (stdio) or `url` (Streamable HTTP). Commands are launched
directly rather than through a shell.

`command`, `args`, and values under `env` support `${NAME}` environment
expansion. The child process receives only `HOME`, `PATH`, `TMPDIR`, `LANG`,
and the explicitly configured `env` values. `startup_timeout_ms` defaults to
30000 and covers process startup plus tool discovery; `timeout_ms` defaults to
10000 for subsequent requests. HTTP `url` and `bearer_token` also support
`${NAME}` expansion; the bearer token is sent only to that endpoint. Remote MCP
endpoints must use HTTPS; HTTP is accepted only for loopback endpoints. Tool output
is limited to 64 KiB before it enters the Agent context. OAuth requires an HTTP
server without `bearer_token`; use `cosh-core mcp logout <server>` to remove its
saved credentials.

Use these short-lived commands to manage configured servers. Their JSON status
contains only `has_credentials`, never access or refresh tokens.

```bash
cosh-core mcp list
cosh-core mcp inspect <server>
cosh-core mcp refresh <server>
cosh-core mcp disconnect <server>
cosh-core mcp connect <server>
```

`inspect` and `refresh` each create a connection, rediscover tools, print the
result, then exit. `disconnect` prevents headless startup from connecting to the
server and removes saved OAuth credentials. `connect` verifies discovery first,
then re-enables a disconnected server.

`[mcp.servers.<name>].allowed_tools` restricts discovery: omit it to expose all
tools, provide a list to expose named tools, or set `[]` to disable every tool
from that server. MCP tools otherwise require approval in `auto`, `balanced`,
`suggest`, and `strict` modes. `[agent].allowed_tools` or `--allowed-tools`
bypasses approval for exact registered tool names such as
`mcp__remote_search__search`.

## cosh-shell Configuration

```toml
[ui]
# Log level
log_level = "warn"

[shell]
# Default shell (auto = auto-detect)
default = "auto"
# Default AI adapter
adapter_default = "cosh-core"
# Analysis mode (smart | auto | manual)
analysis_mode = "smart"
# Approval mode (recommend | auto | trust)
approval_mode = "auto"
```

## Environment Variable Overrides

| Environment Variable | Purpose | Mapped Configuration |
|---------------------|---------|---------------------|
| `COSH_MODEL` | Override active model | `ai.active_model` |
| `COSH_APPROVAL_MODE` | Override approval mode | `agent.approval_mode` |
| `COSH_AI_PROVIDER` | Override active provider | `ai.active_provider` |
| `COSH_OUTPUT_LANGUAGE` | Output language | `ai.output_language` |
| `COSH_MAX_TURNS` | Maximum turns | `agent.max_turns` |
| `COSH_LOG` | Log level (global) | `logging.level` |
| `RUST_LOG` | Rust log filter | — |
| `COSH_SHELL_ADAPTER` | Shell adapter | `shell.adapter_default` |
| `COSH_SHELL_DEBUG` | Maps to debug level | `ui.log_level` |
| `COSH_SHELL_LANG` | Shell language | — |
| `ALIBABA_CLOUD_ACCESS_KEY_ID` | Alibaba Cloud AK | `ai.providers.aliyun.access_key_id` |
| `ALIBABA_CLOUD_ACCESS_KEY_SECRET` | Alibaba Cloud SK | `ai.providers.aliyun.access_key_secret` |
| `DASHSCOPE_API_KEY` | DashScope API Key | Provider resolution chain |

## Log Level Priority

```
COSH_LOG > RUST_LOG > --verbose > config file > default (warn)
```

Valid values: `error`, `warn`, `info`, `debug`, `trace`

## Log Files

```
~/.copilot-shell/logs/
├── cosh-shell.log.2026-06-26    # Daily rotation
├── cosh-core.log.2026-06-26
└── ...
```

## Approval Mode Reference

| Mode | ReadOnly Tools | FileEdit Tools | ShellExec Tools | MCP Tools |
|------|----------------|----------------|-----------------|
| `trust` | Auto-execute | Auto-execute | Auto-execute | Auto-execute |
| `auto` | Auto-execute | Auto-execute | Require approval | Require approval |
| `balanced` | Auto-execute | Require approval | Require approval | Require approval |
| `suggest` | Auto-execute | Require approval | Require approval | Require approval |
| `strict` | Auto-execute | Require approval | Require approval | Require approval |
