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
# 显式缓存开关（仅 DashScope 生效）。
# true  = 显式缓存：主动为 system 和最后一条消息注入 cache_control
#         标记，5 分钟内确定性命中，创建部分按 125% 计费，命中部分按 10% 计费。
# false = 隐式缓存（默认）：自动识别公共前缀，命中率不确定，命中部分按 20% 计费，无法关闭。
# 参考：https://help.aliyun.com/zh/model-studio/context-cache
explicit_cache = false

[agent]
# 审批模式：trust | auto | balanced | suggest | strict
approval_mode = "balanced"
# 单次 Agent 请求内的最大模型轮次
max_turns = 50

[hooks]
enabled = true

[skills]
# 自定义技能搜索路径
custom_paths = []

[mcp.servers.filesystem]
# 本地 stdio Server。
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/workspace"]
# 启动和发现超时；首次 npx 运行可能需要下载包。
startup_timeout_ms = 30000
# 后续 tools/call 请求超时。
timeout_ms = 10000
# 省略时暴露全部已发现工具；设为 [] 时不暴露任何工具。
allowed_tools = ["read_file", "list_directory"]

[mcp.servers.remote-search]
# Streamable HTTP endpoint；不要同时配置 `url` 与 `command`。
url = "https://mcp.example.com/mcp"
# 若使用静态 token 而非 OAuth，取消下行注释：
# bearer_token = "${REMOTE_SEARCH_TOKEN}"
allowed_tools = ["search"]

# OAuth 配置可选；默认使用服务发现和动态客户端注册。
[mcp.servers.remote-search.oauth]
scopes = ["search"]

[session]
# 按工作空间隔离的 provider 对话根目录
persist_dir = "~/.copilot-shell/cosh-core/sessions"
# 设为 false 时仅保留内存会话，输出的 ID 不会用于后续恢复
auto_persist = true

[session.compaction]
enabled = true
auto = true
trigger_ratio = 0.70
emergency_ratio = 0.90
target_ratio = 0.30
preserve_recent_runs = 2

[logging]
level = "warn"
```

项目配置层从 `<workspace>/.copilot-shell/config.toml` 加载，其中 `workspace`
是 `--workspace` 或会话管理请求传入的路径。相对 `session.persist_dir` 从该
工作空间解析，而不是从 Core 进程的启动目录解析。

## Agent 轮次预算

`agent.max_turns` 限制**单次** Agent 请求内消耗的模型轮次，不是整个会话的
总配额：你每发送一条新消息，都会获得新的轮次预算。

| 配置项 | 默认值 | 作用 |
|--------|--------|------|
| `agent.max_turns` | `50` | 单次 Agent 请求允许的模型轮次 |

请求达到上限时，Core 会停止当前 run 并报告
`Agent exceeded max turns (<实际配置的上限>)`，不会静默继续运行。在会话持久化
已启用且成功的情况下，transcript 会在报告该上限之前写入，因此会话保持 active
且可恢复。交互式 shell 随后会提供“继续”或“停止”：选择“继续”会在同一个
provider 对话中启动新的 Agent 请求，并获得同等的配置预算；如果再次达到上限，
仍需再次批准。选择“停止”后，会话仍可由后续手动消息继续。如果 `auto_persist`
关闭，或会话持久化本身失败，则该 run 不可恢复。

可以通过 `[agent]` 下的 `max_turns` 或 `COSH_MAX_TURNS` 环境变量覆盖该预算。
无法解析的 `COSH_MAX_TURNS` 值会被忽略，保留配置文件已解析出的结果。

## 会话压缩

会话跑久以后，早先的回复和工具输出会逐渐占满模型窗口。压缩不会删除持久化的完整
transcript，它只把模型可见的较早前缀换成摘要。常规自动压缩会原样保留
`preserve_recent_runs` 指定的最近 run。遇到 emergency 压力时，Core 先按这个配置
尝试。当前安全边界腾出的空间仍然不够，它才会改为保留一个已完成 run，最后允许
摘要最新的已完成 run。进行中的 run 始终原样保留。

| 配置项 | 默认值 | 作用 |
|--------|--------|------|
| `session.compaction.enabled` | `true` | 启用手动、自动和 emergency 压缩 |
| `session.compaction.auto` | `true` | 在 idle 边界推荐后台压缩 |
| `session.compaction.auto_compact_token_limit` | 未设置 | 可选的绝对自动触发值，并限制在模型可用预算内 |
| `session.compaction.trigger_ratio` | `0.70` | 触发自动压缩的可用历史比例 |
| `session.compaction.emergency_ratio` | `0.90` | 启用 run 内 emergency 保护的比例 |
| `session.compaction.target_ratio` | `0.30` | 压缩后保留历史的尽力目标 |
| `session.compaction.preserve_recent_runs` | `2` | 常规自动压缩原样保留的最近完整 run 数；emergency 保护可依次改为保留一个和不再无条件保留完整 run |
| `session.compaction.model_context_window` | 根据模型确定 | 显式覆盖模型上下文窗口 |
| `session.compaction.model_max_output_tokens` | 见下文 | 显式覆盖模型最大输出规模；同时决定预留的输出预算和真实 provider 请求的 `max_tokens` 上限 |

比例必须满足 `target_ratio <= trigger_ratio <= emergency_ratio`。非法比例组合会
回退到编译时默认值。

### 默认输出预算

回复预算有两个用途。Core 会先从上下文窗口中扣掉这部分空间，再计算可用于会话历史
的额度；它也会把同一个值作为 provider 请求的 `max_tokens` 上限。这样，请求上限
不会超过已经预留的输出空间。未设置 `model_max_output_tokens` 时，Core 使用下表中的
默认值。

| 情况 | 默认值 |
|------|--------|
| 已知模型系列 | `min(模型输出能力, 16384)` |
| 未知模型 | `4096` |

Core 还会把选出的值限制在已解析上下文窗口的一半以内。显式设置
`model_max_output_tokens` 会替换表中的默认值，但仍受这个半窗口上限约束。调高后，
模型可以回复得更长，留给历史的空间会减少。调低后，历史空间会增加，单次回复的
最长长度也会随之缩短。

命令、安全保证以及手动与自动行为差异详见
[会话压缩](shell/session-compaction.md)。

## MCP Server

`cosh-core --headless` 可以启动已配置的 stdio MCP Server，或连接已配置的
Streamable HTTP MCP endpoint，调用
`tools/list`，并将允许的工具注册为 `mcp__<server>__<tool>`。第一版支持
`initialize`、`tools/list` 和 `tools/call`。HTTP Server 可返回 JSON 或 SSE。Streamable
HTTP Server 可通过 `cosh-core mcp login <server>` 使用 OAuth；凭据与配置分开保存。也支持
`2024-11-05` 的旧 HTTP+SSE Server，并会自动 fallback。暂不支持将 cosh-core 作为 MCP Server
对外托管。

MCP Server 定义只从 `/etc/copilot-shell/config.toml` 和
`~/.copilot-shell/config.toml` 读取。为避免检出的项目自动启动任意本地程序，
项目级 `.copilot-shell/config.toml` 中的 MCP 配置会被忽略。每个 Server 必须只配置
`command`（stdio）或 `url`（Streamable HTTP）之一。命令以直接启动的方式执行，不会经过 shell。

`command`、`args` 与 `env` 中的值支持 `${NAME}` 环境变量展开。子进程仅继承
`HOME`、`PATH`、`TMPDIR`、`LANG`，以及显式配置的 `env` 值。`startup_timeout_ms`
默认 30000，覆盖进程启动和工具发现；后续请求的 `timeout_ms` 默认 10000。HTTP 的 `url`
与 `bearer_token` 也支持 `${NAME}` 展开；Bearer token 只会发送给该 endpoint。远端 MCP
endpoint 必须使用 HTTPS；仅 loopback endpoint 可使用 HTTP。工具输出进入
Agent 上下文前限制为 64 KiB。OAuth 要求 HTTP Server 未配置 `bearer_token`；使用
`cosh-core mcp logout <server>` 可删除已保存的凭据。

使用以下短生命周期命令管理已配置的 Server。JSON 状态只包含 `has_credentials`，
不会包含 access token 或 refresh token。

```bash
cosh-core mcp list
cosh-core mcp inspect <server>
cosh-core mcp refresh <server>
cosh-core mcp disconnect <server>
cosh-core mcp connect <server>
```

`inspect` 和 `refresh` 都会连接 Server、重新发现工具、输出结果后退出。`disconnect`
会阻止 headless 启动时连接该 Server，并删除已保存的 OAuth 凭据。`connect` 会先验证
工具发现成功，再重新启用已断开的 Server。

`[mcp.servers.<name>].allowed_tools` 用于限制发现范围：省略表示暴露全部工具，配置列表表示
仅暴露指定工具，设为 `[]` 则禁用该 Server 的所有工具。其他情况下，MCP 工具在 `auto`、
`balanced`、`suggest` 与 `strict` 模式下需要审批。`[agent].allowed_tools` 或
`--allowed-tools` 可为精确的注册工具名跳过审批，例如 `mcp__remote_search__search`。

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
# Agent 批准的前台命令等待终端输入超过该秒数后被打断（0 = 从不打断）。
# 仅内核证据支撑的等待计时：会话 tty 上的密码提示、分页器与普通 stdin
# 读取。全屏 TUI（vi、top）豁免，管道读取（如 `... | cat`）同样豁免。
# 默认：120。
input_wait_timeout_secs = 120
```

## 审计配置

审计复用现有配置文件，但采用更严格的权威顺序：
`/etc/copilot-shell/config.toml` 包含 `[audit]` 时由系统配置完全决定；否则使用用户配置。
项目 `[audit]` 表会被忽略。

```toml
[audit]
mode = "best_effort" # best_effort | required
retention_days = 30
max_disk_bytes = 1073741824
```

`COSH_AUDIT_DIR` 只覆盖存储根目录。未设置时使用 `$XDG_STATE_HOME/cosh/audit` 或
`~/.local/state/cosh/audit`。失败和保留行为见[审计运维指南](cli/audit.md)。

## 环境变量覆盖

| 环境变量 | 作用 | 对应配置 |
|----------|------|----------|
| `COSH_MODEL` | 覆盖活跃模型 | `ai.active_model` |
| `COSH_APPROVAL_MODE` | 覆盖审批模式 | `agent.approval_mode` |
| `COSH_AI_PROVIDER` | 覆盖活跃提供商 | `ai.active_provider` |
| `COSH_OUTPUT_LANGUAGE` | 输出语言 | `ai.output_language` |
| `COSH_MAX_TURNS` | 最大轮次 | `agent.max_turns` |
| `COSH_SERVICE_SITE` | Coding Plan 和 Token Plan 的内置 endpoint 目录 | — |
| `COSH_LOG` | 日志级别（全局） | `logging.level` |
| `RUST_LOG` | Rust 日志过滤 | — |
| `COSH_SHELL_ADAPTER` | Shell 适配器 | `shell.adapter_default` |
| `COSH_SHELL_INPUT_WAIT_TIMEOUT_SECS` | 输入等待超时（秒） | `shell.input_wait_timeout_secs` |
| `COSH_SHELL_DEBUG` | 映射为 debug 级别 | `ui.log_level` |
| `COSH_SHELL_LANG` | Shell 语言 | — |
| `COSH_AUDIT_DIR` | 统一审计存储根目录 | — |
| `ALIBABA_CLOUD_ACCESS_KEY_ID` | 阿里云 AK | `ai.providers.aliyun.access_key_id` |
| `ALIBABA_CLOUD_ACCESS_KEY_SECRET` | 阿里云 SK | `ai.providers.aliyun.access_key_secret` |
| `DASHSCOPE_API_KEY` | DashScope API Key | provider 解析链 |

`COSH_SERVICE_SITE` 支持 `china`/`cn` 和
`international`/`intl`/`global`。未设置或无法识别时使用中国站目录。该变量只
改变 `/auth` 提供的内置 endpoint，不会改写已保存的 provider URL。旧版
OpenAI-compatible Plan provider 仅在 endpoint 与当前站点目录匹配时恢复为 Plan
专用编辑表单，匹配时忽略末尾斜杠。

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
