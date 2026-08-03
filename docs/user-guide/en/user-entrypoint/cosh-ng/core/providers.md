# Model Providers and Authentication

[中文版](../../../../zh/user-entrypoint/cosh-ng/core/providers.md)

The default cosh-core adapter supports OpenAI-compatible streaming providers
and Alibaba Cloud's AK/SK-authenticated SysOM path. Interactive users should
start with `/auth`; configuration is useful for managed or headless setups.

## Interactive authentication

Run:

```text
/auth
```

The picker includes built-in Coding Plan and Token Plan profiles plus provider
credential forms. Built-in plan endpoints use the China service catalog by
default. Select the international catalog before starting cosh when needed:

```bash
COSH_SERVICE_SITE=international cosh
```

Accepted aliases are `china`/`cn` and
`international`/`intl`/`global`. The setting changes the endpoints offered by
the picker; it does not rewrite a saved custom URL.

## Provider configuration

Provider definitions belong in `/etc/copilot-shell/config.toml` or
`~/.copilot-shell/config.toml`, never project configuration:

```toml
[ai]
active_provider = "dashscope"
active_model = "qwen-plus"

[ai.providers.dashscope]
type = "dashscope"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key = "${DASHSCOPE_API_KEY}"
model = "qwen-plus"
explicit_cache = false

[ai.providers.openai]
type = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
model = "gpt-4o"

[ai.providers.deepseek]
type = "deepseek"
base_url = "https://api.deepseek.com/v1"
api_key = "${DEEPSEEK_API_KEY}"
model = "deepseek-chat"

[ai.providers.aliyun]
type = "aliyun"
access_key_id = "${ALIBABA_CLOUD_ACCESS_KEY_ID}"
access_key_secret = "${ALIBABA_CLOUD_ACCESS_KEY_SECRET}"
security_token = "${ALIBABA_CLOUD_SECURITY_TOKEN}"
model = "qwen-plus"
```

For an ECS RAM role, set `type = "aliyun"` and
`auth_source = "ecs_ram_role"` instead of storing static AK/SK values.

## Provider profiles

| `type` | Protocol behavior |
|---|---|
| `dashscope` | OpenAI-compatible request with Qwen reasoning-content handling |
| `openai` | OpenAI token-field conventions |
| `deepseek` | OpenAI-compatible request with reasoning-content handling |
| `aliyun` | Alibaba Cloud request signing with static credentials or ECS RAM role |
| other value | Generic OpenAI-compatible profile |

All model output is streamed. `extra_params` may add provider-specific request
fields; do not use it to override security-sensitive transport configuration.

## Caching and output limits

DashScope uses implicit context caching when `explicit_cache` is `false` or
omitted. Set it to `true` to add explicit cache markers for deterministic
prefix reuse. Explicit and implicit modes have different creation and hit
prices, so confirm the current DashScope billing policy before enabling it.
When a provider reports cache usage, cosh-core accumulates `cached_tokens` and
records it as `session.tokens.cached` in per-turn SLS telemetry.

For recognized model families, cosh-core selects a model-aware maximum output
budget instead of applying the old fixed 4096-token cap. This especially helps
reasoning and long-output models. Unknown models keep the conservative fallback;
the configured model name sent to the provider is never rewritten.

## Resolution order

1. System, user, and permitted project preferences are layered.
2. `COSH_AI_PROVIDER`, `COSH_MODEL`, and `COSH_OUTPUT_LANGUAGE` override the
   corresponding active selections.
3. Provider fields resolve from the selected provider definition.
4. OpenAI-compatible base URL and key fall back to `OPENAI_BASE_URL`,
   `DASHSCOPE_API_KEY`, then `OPENAI_API_KEY` where applicable.
5. Aliyun credentials fall back to the standard
   `ALIBABA_CLOUD_ACCESS_KEY_*` and security-token variables.

`--model <name>` overrides the model only; it does not select a provider with
the same name. Use `COSH_AI_PROVIDER` or `ai.active_provider` to switch provider.

If required credentials are unavailable, an interactive core sends an
`auth_required` control request. A standalone headless client must implement
that exchange or configure credentials before startup.

See [Configuration](../configuration.md) for layer restrictions and secret
handling, and [Headless mode](headless-mode.md) for the control protocol.
