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

[session]
# Session persistence directory (relative to ~/.copilot-shell/)
persist_dir = "sessions"
auto_persist = true

[logging]
level = "warn"
```

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

| Mode | ReadOnly Tools | FileEdit Tools | ShellExec Tools |
|------|----------------|----------------|-----------------|
| `trust` | Auto-execute | Auto-execute | Auto-execute |
| `auto` | Auto-execute | Auto-execute | Require approval |
| `balanced` | Auto-execute | Require approval | Require approval |
| `suggest` | Auto-execute | Require approval | Require approval |
| `strict` | Auto-execute | Require approval | Require approval |
