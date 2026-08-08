# cosh-ng 配置

[English](../../../en/user-entrypoint/cosh-ng/configuration.md)

先使用默认值启动。交互式设置使用 `/auth`、`/mode` 和 `/config language`；需要持久化或共享设置时再编辑 TOML。

## 文件与权限

| 文件 | 读取者 | 作用域 |
|---|---|---|
| `/etc/copilot-shell/config.toml` | `cosh-core` 和审计 | 管理员默认值 |
| `~/.copilot-shell/config.toml` | `cosh-core` 和 `cosh-shell` | 用户设置 |
| `<workspace>/.copilot-shell/config.toml` | `cosh-core` | 项目运行偏好 |

Core 按系统 → 用户 → 项目的顺序合并配置。项目配置可以设置 Agent、Hook、Skill、会话、`active_model` 和回答语言偏好，但会忽略 `active_provider`、Provider 定义、MCP server 和项目审计设置。项目 Hook 仍需在交互式 Shell 中运行 `/hooks trust-project` 才会执行。`cosh-shell` 只读取用户文件，不读取系统或项目文件。

## 最小用户配置

```toml
[ai]
active_provider = "dashscope"
active_model = "qwen3.7-plus"
output_language = "en"

[ai.providers.dashscope]
type = "dashscope"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key = "${DASHSCOPE_API_KEY}"
model = "qwen3.7-plus"

[agent]
approval_mode = "recommend"
max_turns = 50
max_tool_calls_per_turn = 10

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

优先使用环境变量或 `/auth`，不要把原始 secret 写入 TOML。Provider 选择见 [Providers](core/providers.md)。

## 审批与轮次预算

Core 审批模式适用于直接集成：

| 模式 | ReadOnly | FileEdit | Shell、network、MCP、external |
|---|---|---|---|
| `trust` | 执行 | 执行 | 执行 |
| `auto` | 执行 | 执行 | 询问 |
| `recommend` | 执行 | 询问 | 询问 |

Core 和 Shell 统一使用 `recommend`、`auto` 和 `trust`。已有的 `balanced`、
`suggest` 和 `strict` 会按 `recommend` 读取；非法配置值也会回退到
`recommend`。`agent.max_turns` 限制一次 Agent 请求（默认 `50`），
`max_tool_calls_per_turn` 默认 `10`。每次新 prompt 都会重新开始轮次预算。

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

保持 `target_ratio <= trigger_ratio <= emergency_ratio`。压缩只改变模型可见的历史，持久化的完整 transcript 不变。设置 `auto_persist = false` 可关闭该进程的会话恢复。

## MCP 与其他可选部分

MCP client 只能定义在系统或用户配置中。每个 server 使用 `command`（stdio）或 `url`（Streamable HTTP）之一；省略 `allowed_tools` 表示全部工具，`[]` 表示不暴露工具。示例、OAuth 和生命周期命令见[接入 MCP server](mcp.md)。

只添加实际需要的 Shell 建议和健康检查：

```toml
[shell.recommendations]
enabled = true
bash_history = false

[health]
enabled = true
role = "web-server"
critical_mounts = ["/", "/var"]

[[health.services]]
name = "nginx"
expected = "active"
```

`analysis_mode` 支持 `smart`、`auto`、`manual`；Shell 审批支持 `recommend`、`auto`、`trust`。`health.services.expected` 支持 `active` 或 `inactive`。

## 审计设置

系统文件包含 `[audit]` 表时，以系统设置为准；否则使用用户设置。项目审计表会被忽略。

```toml
[audit]
mode = "best_effort"         # best_effort | required
retention_days = 30
max_disk_bytes = 1073741824
```

`retention_days` 和 `max_disk_bytes` 必须大于零。存储根目录默认为 `$XDG_STATE_HOME/cosh/audit` 或 `~/.local/state/cosh/audit`；设置绝对路径的 `COSH_AUDIT_DIR` 可以覆盖它。

## 环境变量覆盖

| 变量 | 作用 |
|---|---|
| `COSH_AI_PROVIDER`、`COSH_MODEL`、`COSH_OUTPUT_LANGUAGE` | Core Provider、模型和回答语言 |
| `COSH_APPROVAL_MODE`、`COSH_MAX_TURNS` | Core 审批和单次请求轮次预算 |
| `DASHSCOPE_API_KEY`、`OPENAI_API_KEY`、`OPENAI_BASE_URL` | OpenAI-compatible 凭据和 URL 回退 |
| `ALIBABA_CLOUD_ACCESS_KEY_ID`、`ALIBABA_CLOUD_ACCESS_KEY_SECRET`、`ALIBABA_CLOUD_SECURITY_TOKEN` | Aliyun 凭据回退 |
| `COSH_SHELL_DEFAULT_SHELL`、`COSH_SHELL_ADAPTER`、`COSH_SHELL_ANALYSIS_MODE`、`COSH_SHELL_APPROVAL_MODE` | 交互式 Shell 选择 |
| `COSH_SHELL_LANG`、`COSH_SHELL_AI`、`COSH_SHELL_INPUT_WAIT_TIMEOUT_SECS` | Shell 语言、AI 开关和输入等待超时 |
| `COSH_RECOMMENDATIONS_BASH_HISTORY` | 允许使用 Bash history 生成建议 |
| `COSH_LOG`、`RUST_LOG` | 日志过滤（`COSH_LOG` 优先） |
| `COSH_AUDIT_DIR` | 审计存储根目录 |

相关二进制支持时，环境变量优先于配置文件。日志在 `~/.copilot-shell/logs/` 下按日轮转，旧文件保留七天。
