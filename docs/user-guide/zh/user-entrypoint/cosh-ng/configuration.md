# 配置

cosh-ng 的三个二进制共享配置文件 `~/.copilot-shell/config.toml`。支持环境变量
覆盖和 CLI 参数优先。

## 配置文件位置

配置按以下优先级加载（从高到低）：

1. `.copilot-shell/config.toml`（项目级，当前目录）
2. `~/.copilot-shell/config.toml`（用户级）
3. `/etc/copilot-shell/config.toml`（系统级）

## cosh-core 配置

```toml
[ai]
# 活跃的模型标识
active_model = "qwen-plus"
# 输出语言（可选）
output_language = "zh"

[ai.providers.aliyun]
type = "aliyun"
access_key_id = ""        # 或通过 ALIBABA_CLOUD_ACCESS_KEY_ID
access_key_secret = ""    # 或通过 ALIBABA_CLOUD_ACCESS_KEY_SECRET
model = "qwen-plus"

[ai.providers.dashscope]
type = "dashscope"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key = ""              # 或通过 DASHSCOPE_API_KEY
model = "qwen-plus"

[agent]
# 审批模式：trust | auto | balanced | suggest | strict
approval_mode = "balanced"
# 最大对话轮次
max_turns = 20

[hooks]
enabled = true

[skills]
# 自定义技能搜索路径
custom_paths = []

[mcp.servers.filesystem]
# 当前版本仅支持 stdio Server。
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
# 启动和发现超时；首次 npx 运行可能需要下载包。
startup_timeout_ms = 30000
# 后续 tools/call 请求超时。
timeout_ms = 10000
# 省略时暴露全部已发现工具；设为 [] 时不暴露任何工具。
allowed_tools = ["read_file", "list_directory"]

[mcp.servers.filesystem.env]
FILESYSTEM_API_KEY = "${FILESYSTEM_API_KEY}"

[session]
# 按工作空间隔离的 provider 对话根目录
persist_dir = "~/.copilot-shell/cosh-core/sessions"
# 设为 false 时仅保留内存会话，输出的 ID 不会用于后续恢复
auto_persist = true

[logging]
level = "warn"
```

项目配置层从 `<workspace>/.copilot-shell/config.toml` 加载，其中 `workspace`
是 `--workspace` 或会话管理请求传入的路径。相对 `session.persist_dir` 从该
工作空间解析，而不是从 Core 进程的启动目录解析。

## MCP stdio Server

`cosh-core --headless` 可以启动已配置的 stdio MCP Server，调用
`tools/list`，并将允许的工具注册为 `mcp__<server>__<tool>`。第一版支持
`initialize`、`tools/list` 和 `tools/call`；暂不支持 HTTP/SSE transport、OAuth，
也不能将 cosh-core 作为 MCP Server 对外托管。

MCP Server 定义只从 `/etc/copilot-shell/config.toml` 和
`~/.copilot-shell/config.toml` 读取。为避免检出的项目自动启动任意本地程序，
项目级 `.copilot-shell/config.toml` 中的 MCP 配置会被忽略。命令以直接启动的方式执行，
不会经过 shell。

`command`、`args` 与 `env` 中的值支持 `${NAME}` 环境变量展开。子进程仅继承
`HOME`、`PATH`、`TMPDIR`、`LANG`，以及显式配置的 `env` 值。`startup_timeout_ms`
默认 30000，覆盖进程启动和工具发现；后续请求的 `timeout_ms` 默认 10000。工具输出进入
Agent 上下文前限制为 64 KiB。

在 `auto`、`balanced`、`suggest` 与 `strict` 模式下，所有 MCP 工具都需要用户审批；
只有已有的 `trust` 模式会跳过审批。`allowed_tools` 可限制发现范围：省略表示暴露全部工具，
配置列表表示仅暴露指定工具，设为 `[]` 则禁用该 Server 的所有工具。

## cosh-shell 配置

```toml
[ui]
# 日志级别
log_level = "warn"

[shell]
# 默认 shell（auto = 自动检测）
default = "auto"
# 默认 AI 适配器
adapter_default = "cosh-core"
# 分析模式（smart | auto | manual）
analysis_mode = "smart"
# 审批模式（recommend | auto | trust）
approval_mode = "auto"
```

## 环境变量覆盖

| 环境变量 | 作用 | 对应配置 |
|----------|------|----------|
| `COSH_MODEL` | 覆盖活跃模型 | `ai.active_model` |
| `COSH_APPROVAL_MODE` | 覆盖审批模式 | `agent.approval_mode` |
| `COSH_AI_PROVIDER` | 覆盖活跃提供商 | `ai.active_provider` |
| `COSH_OUTPUT_LANGUAGE` | 输出语言 | `ai.output_language` |
| `COSH_MAX_TURNS` | 最大轮次 | `agent.max_turns` |
| `COSH_LOG` | 日志级别（全局） | `logging.level` |
| `RUST_LOG` | Rust 日志过滤 | — |
| `COSH_SHELL_ADAPTER` | Shell 适配器 | `shell.adapter_default` |
| `COSH_SHELL_DEBUG` | 映射为 debug 级别 | `ui.log_level` |
| `COSH_SHELL_LANG` | Shell 语言 | — |
| `ALIBABA_CLOUD_ACCESS_KEY_ID` | 阿里云 AK | `ai.providers.aliyun.access_key_id` |
| `ALIBABA_CLOUD_ACCESS_KEY_SECRET` | 阿里云 SK | `ai.providers.aliyun.access_key_secret` |
| `DASHSCOPE_API_KEY` | DashScope API Key | provider 解析链 |

## 日志级别优先级

```
COSH_LOG > RUST_LOG > --verbose > config file > default (warn)
```

合法值：`error`、`warn`、`info`、`debug`、`trace`

## 日志文件

```
~/.copilot-shell/logs/
├── cosh-shell.log.2026-06-26    # 按天轮转
├── cosh-core.log.2026-06-26
└── ...
```

## 审批模式说明

| 模式 | ReadOnly 工具 | FileEdit 工具 | ShellExec 工具 | MCP 工具 |
|------|---------------|---------------|----------------|
| `trust` | 自动执行 | 自动执行 | 自动执行 | 自动执行 |
| `auto` | 自动执行 | 自动执行 | 需要审批 | 需要审批 |
| `balanced` | 自动执行 | 需要审批 | 需要审批 | 需要审批 |
| `suggest` | 自动执行 | 需要审批 | 需要审批 | 需要审批 |
| `strict` | 自动执行 | 需要审批 | 需要审批 | 需要审批 |
