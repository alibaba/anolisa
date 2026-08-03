# 模型 Provider 与认证

[English](../../../../en/user-entrypoint/cosh-ng/core/providers.md)

默认 cosh-core 适配器支持 OpenAI-compatible 流式 provider，也支持使用 AK/SK 认证的
Alibaba Cloud SysOM。交互式使用从 `/auth` 开始。托管或 headless 环境可以直接写配置文件。

## 交互式认证

运行以下命令。

```text
/auth
```

认证菜单包含内置 Coding Plan、Token Plan 和 provider 凭据表单。内置计划默认使用
中国站服务目录。需要国际站时，在启动 cosh 前设置环境变量。

```bash
COSH_SERVICE_SITE=international cosh
```

可用别名包括 `china`/`cn` 和 `international`/`intl`/`global`。该设置只改变认证菜单中
的 endpoint，已保存的自定义 URL 不会改变。

## Provider 配置

Provider 定义只能放在 `/etc/copilot-shell/config.toml` 或
`~/.copilot-shell/config.toml`，项目配置不接受这些定义。

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

使用 ECS RAM role 时，设置 `type = "aliyun"` 和 `auth_source = "ecs_ram_role"`，
无需保存静态 AK/SK。

## Provider 类型

| `type` | 协议行为 |
|---|---|
| `dashscope` | 发送 OpenAI-compatible 请求，并处理 Qwen reasoning content |
| `openai` | 使用 OpenAI token 字段约定 |
| `deepseek` | 发送 OpenAI-compatible 请求，并处理 reasoning content |
| `aliyun` | 使用静态凭据或 ECS RAM role 签名 Alibaba Cloud 请求 |
| 其他值 | 使用通用 OpenAI-compatible 协议 |

所有模型输出都使用流式传输。`extra_params` 可以增加 provider-specific 请求字段。
不要用它覆盖安全敏感的传输配置。

## Cache 与输出上限

`explicit_cache` 为 `false` 或省略时，DashScope 使用隐式上下文缓存。设为 `true`
后会添加显式 cache marker，用于复用确定的前缀。显式和隐式模式的创建与
命中价格不同，启用前应确认 DashScope 当前的计费规则。
Provider 返回 cache usage 时，cosh-core 会累计 `cached_tokens`，并在每轮 SLS 记录中
写入 `session.tokens.cached`。

对已识别的模型家族，cosh-core 会按模型选择最大输出预算。旧版本固定使用
4096-token 上限，现在 reasoning 和长输出模型可以使用更合适的预算。未识别的模型仍使用
保守默认值。发送给 provider 的原始模型名不会改变。

## 配置如何生效

1. 依次应用系统、用户和允许的项目偏好。
2. `COSH_AI_PROVIDER`、`COSH_MODEL`、`COSH_OUTPUT_LANGUAGE` 覆盖当前选择。
3. 从已选中的 provider 定义解析其他字段。
4. OpenAI-compatible base URL 和 key 在适用时依次回退到 `OPENAI_BASE_URL`、
   `DASHSCOPE_API_KEY`、`OPENAI_API_KEY`。
5. Aliyun 凭据回退到标准 `ALIBABA_CLOUD_ACCESS_KEY_*` 和 security-token 变量。

`--model <name>` 只覆盖模型名。切换 provider 请使用 `COSH_AI_PROVIDER` 或
`ai.active_provider`。

缺少必要凭据时，交互式 Core 会发送 `auth_required` control request。独立 headless client
需要处理该请求，或者在启动前配置凭据。

配置层限制和 secret 处理见[配置](../configuration.md)，control protocol 见
[Headless 模式](headless-mode.md)。
