# LLM Providers

cosh-core connects to multiple LLM providers via the OpenAI-compatible API protocol. All providers use streaming SSE output and support function calling.

## Supported Providers

| Provider Type | Profile | Description |
|--------------|---------|-------------|
| `dashscope` | DashScope | Alibaba Cloud Bailian (Qwen series), supports thinking |
| `aliyun` | SysOM | Alibaba Cloud AK/SK signature authentication (ROA style) |
| `openai` | OpenAI | OpenAI official API, uses `max_completion_tokens` |
| `deepseek` | DeepSeek | DeepSeek API, supports thinking |
| Other | Generic | Any OpenAI-compatible endpoint |

## Configuration

Configure providers in `~/.copilot-shell/config.toml`:

```toml
[ai]
active_model = "qwen-plus"

[ai.providers.dashscope]
type = "dashscope"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key = ""    # Or via DASHSCOPE_API_KEY environment variable
model = "qwen-plus"

[ai.providers.aliyun]
type = "aliyun"
access_key_id = ""      # Or ALIBABA_CLOUD_ACCESS_KEY_ID
access_key_secret = ""  # Or ALIBABA_CLOUD_ACCESS_KEY_SECRET
model = "qwen-plus"

[ai.providers.openai]
type = "openai"
base_url = "https://api.openai.com/v1"
api_key = ""    # Or OPENAI_API_KEY
model = "gpt-4o"

[ai.providers.deepseek]
type = "deepseek"
base_url = "https://api.deepseek.com/v1"
api_key = ""
model = "deepseek-chat"
```

## Provider Profile Differences

Behavioral differences across profiles in API requests:

| Profile | max_tokens Field | Thinking Field | Authentication |
|---------|-----------------|----------------|----------------|
| Generic | `max_tokens` | — | Bearer token |
| DashScope | `max_tokens` | `reasoning_content` | Bearer token |
| OpenAI | `max_completion_tokens` | — | Bearer token |
| DeepSeek | `max_tokens` | `reasoning_content` | Bearer token |
| SysOM (aliyun) | — | — | AK/SK signature |

## Runtime Switching

Dynamically switch models via JSONL control protocol:

```json
{"type":"control_request","request_id":"sw-1","request":{"subtype":"switch_model","model":"qwen-max"}}
```

Or override via CLI arguments:

```bash
cosh-core --headless --model qwen-max
```

Environment variable override:

```bash
COSH_MODEL=qwen-max cosh-core --headless
```

## Authentication Priority

1. CLI `--model` argument → selects corresponding provider config
2. Environment variables (`DASHSCOPE_API_KEY`, `ALIBABA_CLOUD_ACCESS_KEY_*`)
3. Values in config.toml `[ai.providers.<name>]`
4. All empty → triggers interactive authentication flow (sends `auth_required` to Shell)

## Generation Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `max_tokens` | 4096 | Maximum generated tokens |
| `temperature` | — | Sampling temperature (uses provider default if not set) |
| `stream` | true | Always stream output |

Custom parameters can be passed via `extra_params` in the `[ai.providers.<name>]` section of config.toml.
