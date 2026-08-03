# cosh-ng Configuration

[中文版](../../../zh/user-entrypoint/cosh-ng/configuration.md)

Start with defaults and use `/auth`, `/mode`, and `/config language` for common
interactive changes. Edit TOML when configuration must persist or be shared.

## Files and authority

| File | cosh-core | cosh-shell | Trust level |
|---|---|---|---|
| `/etc/copilot-shell/config.toml` | System defaults | Not read | Administrator |
| `~/.copilot-shell/config.toml` | User overrides | User UI/shell settings | User |
| `<workspace>/.copilot-shell/config.toml` | Project overrides for non-secret runtime preferences | Not read | Untrusted project input |

Core applies system, then user, then project values. Project configuration may
change common Agent, hook, Skill, session, and model-preference fields, but it
cannot define providers, select `active_provider`, define MCP servers, or own
audit authority. This prevents a checkout from introducing credentials or
starting external MCP programs.

The workspace is the canonical path sent by cosh-shell, not necessarily the
launcher's current directory. Relative session paths and project configuration
resolve from that workspace.

## Minimal user configuration

```toml
[ai]
active_provider = "dashscope"
active_model = "qwen-plus"
output_language = "en"

[ai.providers.dashscope]
type = "dashscope"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key = "${DASHSCOPE_API_KEY}"
model = "qwen-plus"
# DashScope prompt caching. false uses implicit caching; true adds explicit
# cache_control markers. Check the current provider policy before enabling it.
explicit_cache = false

[agent]
approval_mode = "balanced"
max_turns = 50
max_tool_calls_per_turn = 10

[hooks]
enabled = true

[skills]
custom_paths = ["~/team-skills"]

[session]
auto_persist = true
persist_dir = "~/.copilot-shell/cosh-core/sessions"

[logging]
level = "warn"

[ui]
language = "auto"
log_level = "warn"

[shell]
default = "auto"
adapter_default = "cosh-core"
analysis_mode = "smart"
approval_mode = "auto"
```

Prefer environment expansion or `/auth` over writing a raw secret into TOML.
`/auth` also supports built-in Coding Plan and Token Plan profiles.

## Approval and turn limits

Core approval modes apply to direct headless integrations:

| Core mode | ReadOnly | FileEdit | Shell / network / MCP / external |
|---|---|---|---|
| `trust` | Run | Run | Run |
| `auto` | Run | Run | Ask |
| `balanced`, `suggest`, `strict` | Run | Ask | Ask |

cosh-shell exposes `recommend`, `auto`, and `trust`. `recommend` maps to strict
core behavior; entering trust requires `/mode approval trust confirm`.

`agent.max_turns` limits one Agent request, not the whole session. The default
is 50. If a persisted interactive run reaches the limit, cosh can ask whether
to continue with another budget in the same provider conversation.
`max_tool_calls_per_turn` defaults to 10.

## Sessions and compaction

```toml
[session]
auto_persist = true
persist_dir = "~/.copilot-shell/cosh-core/sessions"

[session.compaction]
enabled = true
auto = true
trigger_ratio = 0.70
emergency_ratio = 0.90
target_ratio = 0.30
preserve_recent_runs = 2
# auto_compact_token_limit = 89600
# model_context_window = 128000
# model_max_output_tokens = 8192
```

Ratios must satisfy `target_ratio <= trigger_ratio <= emergency_ratio`.
Invalid or non-finite project values fall back to compiled defaults. Compaction
changes only the model-visible projection; the persisted transcript remains
complete. Under emergency pressure, Core can progressively retain fewer
completed runs, but the active run remains verbatim.

`model_max_output_tokens` sets both the reply space reserved in the context
window and the provider request's `max_tokens` cap. Without an override, known
models use `min(model output capability, 16384)` and unknown models use `4096`;
both are capped at half the context window. See
[Session compaction](shell/session-compaction.md) for commands and detailed
behavior.

## MCP servers

For an end-to-end setup, connection, authentication, and troubleshooting
workflow, follow [Connect an MCP server](mcp.md). This section is the compact
configuration reference.

Define MCP clients only in system or user configuration. Each server sets
exactly one of `command` (stdio) or `url` (Streamable HTTP):

```toml
[mcp.servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
startup_timeout_ms = 30000
timeout_ms = 10000
allowed_tools = ["read_file", "list_directory"]

[mcp.servers.search]
url = "https://mcp.example.com/mcp"
allowed_tools = ["query"]

[mcp.servers.search.oauth]
scopes = ["search"]
```

- Commands launch directly, not through a shell, and receive only a small base
  environment plus explicitly configured `env` values.
- `${NAME}` expansion works in command fields, environment values, HTTP URLs,
  and bearer tokens.
- Remote URLs require HTTPS; loopback HTTP is allowed.
- Omit `allowed_tools` to expose all discovered tools; use `[]` to expose none.
- Both stdio and HTTP servers receive the canonical workspace through MCP
  roots.
- Output entering Agent context is limited to 64 KiB.

Use `/mcp` inside cosh to inspect and manage configured servers. OAuth tokens
are stored separately from TOML. `disconnect` disables startup connection and
removes saved OAuth credentials; `connect` verifies discovery before enabling
the server again.

## Shell UI, recommendations, and health

```toml
[ui]
language = "auto"            # auto | en-US | zh-CN
startup_banner = true
startup_hooks = false
debug = false
log_level = "warn"

[shell]
default = "auto"             # auto | bash | zsh
adapter_default = "cosh-core"
analysis_mode = "smart"      # smart | auto | manual
approval_mode = "auto"       # recommend | auto | trust
trusted_commands = []
trusted_project_roots = []
# Interrupt an approved foreground command after this many seconds waiting
# for terminal input. Use 0 to disable; fullscreen TUIs and pipelines are exempt.
input_wait_timeout_secs = 120

[shell.recommendations]
enabled = true
bash_history = false

[health]
enabled = true
role = "web-server"
memory_sensitive = false
critical_mounts = ["/", "/var"]
verbose = false

[[health.services]]
name = "nginx"
expected = "active"          # active | inactive
```

Recommendations are stored locally and can be inspected or cleared with
`/recommendations`. Bash history is opt-in. Project hook trust is stored
separately; adding a path to project configuration does not grant trust.

## Audit

Audit configuration is authoritative from the system file when it contains an
`[audit]` table; otherwise the user table is used. Project audit tables are
ignored.

```toml
[audit]
mode = "best_effort"         # best_effort | required
retention_days = 30
max_disk_bytes = 1073741824
```

`COSH_AUDIT_DIR` overrides the storage root. The default is
`$XDG_STATE_HOME/cosh/audit` or `~/.local/state/cosh/audit`.

## Environment overrides

| Variable | Purpose |
|---|---|
| `COSH_AI_PROVIDER`, `COSH_MODEL` | Active provider and model |
| `COSH_APPROVAL_MODE`, `COSH_MAX_TURNS` | Core approval and per-request turn budget |
| `COSH_OUTPUT_LANGUAGE` | Core response language |
| `DASHSCOPE_API_KEY`, `OPENAI_API_KEY`, `OPENAI_BASE_URL` | OpenAI-compatible provider resolution |
| `ALIBABA_CLOUD_ACCESS_KEY_ID`, `ALIBABA_CLOUD_ACCESS_KEY_SECRET`, `ALIBABA_CLOUD_SECURITY_TOKEN` | Aliyun provider credentials |
| `COSH_SERVICE_SITE` | `/auth` plan catalog: China by default, or `international` |
| `COSH_SHELL_DEFAULT_SHELL`, `COSH_SHELL_ADAPTER` | Interactive shell and adapter |
| `COSH_SHELL_ANALYSIS_MODE`, `COSH_SHELL_APPROVAL_MODE` | Interactive modes |
| `COSH_SHELL_INPUT_WAIT_TIMEOUT_SECS` | Approved foreground-command input-wait timeout |
| `COSH_SHELL_LANG`, `COSH_SHELL_AI` | UI language and AI on/off |
| `COSH_SHELL_DEBUG` | Map shell logging to debug level |
| `COSH_RECOMMENDATIONS_BASH_HISTORY` | Opt into local bash-history recommendations |
| `COSH_LOG`, `RUST_LOG` | Log filtering, in that priority order |
| `COSH_AUDIT_DIR` | Audit storage root |

Configuration follows environment/CLI overrides where the relevant binary
supports them. Logs rotate daily under `~/.copilot-shell/logs/` and old files
are retained for seven days.
