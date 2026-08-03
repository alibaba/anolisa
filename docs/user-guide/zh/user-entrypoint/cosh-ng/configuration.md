# cosh-ng 配置

[English](../../../en/user-entrypoint/cosh-ng/configuration.md)

先用默认值启动。常见设置可以通过 `/auth`、`/mode` 和 `/config language` 修改。
配置需要持续生效或与团队共享时，再编辑 TOML。

## 配置文件与权限

| 文件 | cosh-core | cosh-shell | 信任级别 |
|---|---|---|---|
| `/etc/copilot-shell/config.toml` | 系统默认值 | 不读取 | 管理员 |
| `~/.copilot-shell/config.toml` | 用户设置 | UI 和 Shell 设置 | 用户 |
| `<workspace>/.copilot-shell/config.toml` | 项目内的非敏感运行偏好 | 不读取 | 不受信任的项目输入 |

Core 依次读取系统、用户和项目配置。项目配置可以修改常用的 Agent、Hook、
Skill、会话和模型偏好。Provider、`active_provider`、MCP server 和审计权限不接受
项目配置，避免仅因 checkout 了某个项目就引入凭据或启动外部程序。

工作空间由 cosh-shell 启动时确定。它与启动器的当前目录可能不同。相对会话路径和
项目配置都从这个工作空间解析。

## 最小用户配置

```toml
[ai]
active_provider = "dashscope"
active_model = "qwen-plus"
output_language = "zh"

[ai.providers.dashscope]
type = "dashscope"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key = "${DASHSCOPE_API_KEY}"
model = "qwen-plus"
# DashScope prompt cache。false 使用隐式缓存，true 添加显式
# cache_control 标记。启用前请查看 provider 当前规则。
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

优先使用环境变量或 `/auth`，避免把原始 secret 写入 TOML。`/auth` 也支持内置的
Coding Plan 和 Token Plan。

## 审批与单次请求上限

Core 审批模式适用于直接集成 headless 接口的客户端。

| Core mode | ReadOnly | FileEdit | Shell / network / MCP / external |
|---|---|---|---|
| `trust` | 执行 | 执行 | 执行 |
| `auto` | 执行 | 执行 | 询问 |
| `balanced`、`suggest`、`strict` | 执行 | 询问 | 询问 |

cosh-shell 提供 `recommend`、`auto` 和 `trust`。`recommend` 对应 Core 的严格模式。
进入 `trust` 前必须执行 `/mode approval trust confirm`。

`agent.max_turns` 只限制一次 Agent 请求，对整个会话没有影响。默认值是 50。
持久对话中的一次任务达到上限时，cosh 可以询问是否在同一 provider 对话中继续。
`max_tool_calls_per_turn` 的默认值是 10。

## 会话与压缩

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

比例必须满足 `target_ratio <= trigger_ratio <= emergency_ratio`。项目配置中的无效值会
回退到编译时默认值。压缩只调整模型可见的对话内容，持久保存的完整记录不会改变。
紧急压缩可以逐步减少原样保留的已完成任务，正在执行的任务始终保持原文。

`model_max_output_tokens` 同时决定上下文窗口预留的回复空间和模型请求的 `max_tokens`
上限。没有显式设置时，已知模型使用 `min(模型输出能力, 16384)`，未知模型使用 `4096`，
两者都不会超过上下文窗口的一半。命令和详细行为见[会话压缩](shell/session-compaction.md)。

## MCP server

从配置、连接到认证和排障的完整步骤见[接入 MCP server](mcp.md)。本节只保留配置字段
参考。

只在系统或用户配置中定义 MCP client。每个 server 只能使用 `command`（stdio）或
`url`（Streamable HTTP）中的一种方式。

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

- Command 直接启动，不经过 Shell，只接收基础环境和明确配置的 `env` 值。
- `command` 字段、环境值、HTTP URL 和 bearer token 支持 `${NAME}` 展开。
- 远程 URL 必须使用 HTTPS；允许 loopback HTTP。
- 省略 `allowed_tools` 表示暴露所有发现工具，`[]` 表示全部不暴露。
- Stdio 和 HTTP server 都会通过 MCP roots 接收当前工作空间。
- 进入 Agent 上下文的输出上限是 64 KiB。

在 cosh 中使用 `/mcp` 检查和管理已配置的 server。OAuth token 与 TOML 分开存储。
`disconnect` 会禁用启动时连接，并移除已保存的 OAuth 凭据。`connect` 会先验证发现结果，
再重新启用 server。

## Shell 界面、输入建议与健康检查

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
# 已批准前台命令等待终端输入超过该秒数后中断。
# 0 表示禁用；全屏 TUI 和管道读取不计时。
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

输入建议保存在本地，可以通过 `/recommendations` 查看或清理。Bash history 默认不会参与
生成建议。项目 Hook 信任单独保存。在项目配置中添加路径不会授予信任。

## 审计

系统配置包含 `[audit]` 时，审计设置完全以它为准。系统配置没有该表时，使用
用户配置。项目中的审计设置会被忽略。

```toml
[audit]
mode = "best_effort"         # best_effort | required
retention_days = 30
max_disk_bytes = 1073741824
```

`COSH_AUDIT_DIR` 覆盖存储根目录。默认位置为 `$XDG_STATE_HOME/cosh/audit` 或
`~/.local/state/cosh/audit`。

## 环境变量覆盖

| 变量 | 用途 |
|---|---|
| `COSH_AI_PROVIDER`、`COSH_MODEL` | 当前 provider 和模型 |
| `COSH_APPROVAL_MODE`、`COSH_MAX_TURNS` | Core 审批模式和单次请求轮次上限 |
| `COSH_OUTPUT_LANGUAGE` | Core 回答语言 |
| `DASHSCOPE_API_KEY`、`OPENAI_API_KEY`、`OPENAI_BASE_URL` | OpenAI-compatible provider 连接信息 |
| `ALIBABA_CLOUD_ACCESS_KEY_ID`、`ALIBABA_CLOUD_ACCESS_KEY_SECRET`、`ALIBABA_CLOUD_SECURITY_TOKEN` | Aliyun provider 凭据 |
| `COSH_SERVICE_SITE` | `/auth` 服务目录，默认使用中国站，也可设为 `international` |
| `COSH_SHELL_DEFAULT_SHELL`、`COSH_SHELL_ADAPTER` | 交互式 Shell 和适配器 |
| `COSH_SHELL_ANALYSIS_MODE`、`COSH_SHELL_APPROVAL_MODE` | 交互模式 |
| `COSH_SHELL_INPUT_WAIT_TIMEOUT_SECS` | 已批准前台命令的输入等待超时 |
| `COSH_SHELL_LANG`、`COSH_SHELL_AI` | UI 语言和 AI 开关 |
| `COSH_SHELL_DEBUG` | 将 Shell 日志级别设为 debug |
| `COSH_RECOMMENDATIONS_BASH_HISTORY` | 允许使用本地 Bash history 生成输入建议 |
| `COSH_LOG`、`RUST_LOG` | 日志过滤，优先级按表中顺序 |
| `COSH_AUDIT_DIR` | 审计存储根目录 |

相应二进制支持时，环境变量和 CLI 参数优先于配置文件。日志会在
`~/.copilot-shell/logs/` 下按日轮转，旧文件保留七天。
