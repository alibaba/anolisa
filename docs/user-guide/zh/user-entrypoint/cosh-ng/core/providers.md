# LLM 提供商

cosh-core 通过 OpenAI 兼容 API 协议对接多家 LLM 提供商。所有提供商均使用流式 SSE 输出，支持 function calling。

## 支持的提供商

| 提供商类型 | Profile | 说明 |
|------------|---------|------|
| `dashscope` | DashScope | 阿里云百炼（通义千问系列），支持 thinking |
| `aliyun` | SysOM | 阿里云 AK/SK 签名认证（ROA 风格） |
| `openai` | OpenAI | OpenAI 官方 API，使用 `max_completion_tokens` |
| 其他 | Generic | 任意 OpenAI 兼容端点 |

## 配置

在 `~/.copilot-shell/config.toml` 中配置提供商：

```toml
[ai]
active_model = "qwen-plus"

[ai.providers.dashscope]
type = "dashscope"
base_url = "https://dashscope.aliyuncs.com/compatible-mode/v1"
api_key = ""    # 或通过 DASHSCOPE_API_KEY 环境变量
model = "qwen-plus"

[ai.providers.aliyun]
type = "aliyun"
access_key_id = ""      # 或 ALIBABA_CLOUD_ACCESS_KEY_ID
access_key_secret = ""  # 或 ALIBABA_CLOUD_ACCESS_KEY_SECRET
model = "qwen-plus"

[ai.providers.openai]
type = "openai"
base_url = "https://api.openai.com/v1"
api_key = ""    # 或 OPENAI_API_KEY
model = "gpt-4o"
```

## Provider Profile 差异

不同 Profile 在 API 请求中的行为差异：

| Profile | max_tokens 字段 | thinking 字段 | 认证方式 |
|---------|----------------|---------------|----------|
| Generic | `max_tokens` | — | Bearer token |
| DashScope | `max_tokens` | `reasoning_content` | Bearer token |
| OpenAI | `max_completion_tokens` | — | Bearer token |
| SysOM (aliyun) | — | — | AK/SK 签名 |

## 运行时切换

通过 JSONL 控制协议动态切换模型：

```json
{"type":"control_request","request_id":"sw-1","request":{"subtype":"switch_model","model":"qwen-max"}}
```

或通过 CLI 参数覆盖：

```bash
cosh-core --headless --model qwen-max
```

环境变量覆盖：

```bash
COSH_MODEL=qwen-max cosh-core --headless
```

## 认证优先级

1. CLI `--model` 参数 → 选择对应的 provider 配置
2. 环境变量（`DASHSCOPE_API_KEY`、`ALIBABA_CLOUD_ACCESS_KEY_*`）
3. config.toml 中 `[ai.providers.<name>]` 的配置值
4. 均为空时 → 触发交互式认证流程（向 Shell 发送 `auth_required`）

## 生成参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `max_tokens` | 4096 | 最大生成 token 数 |
| `temperature` | — | 采样温度（不设置则使用提供商默认） |
| `stream` | true | 始终流式输出 |

可通过 config.toml 的 `[ai.providers.<name>]` 段添加 `extra_params` 传递自定义参数。
