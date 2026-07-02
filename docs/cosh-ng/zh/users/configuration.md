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

[session]
# 会话持久化目录（相对于 ~/.copilot-shell/）
persist_dir = "sessions"
auto_persist = true

[logging]
level = "warn"
```

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

| 模式 | ReadOnly 工具 | FileEdit 工具 | ShellExec 工具 |
|------|---------------|---------------|----------------|
| `trust` | 自动执行 | 自动执行 | 自动执行 |
| `auto` | 自动执行 | 自动执行 | 需要审批 |
| `balanced` | 自动执行 | 需要审批 | 需要审批 |
| `suggest` | 自动执行 | 需要审批 | 需要审批 |
| `strict` | 自动执行 | 需要审批 | 需要审批 |
