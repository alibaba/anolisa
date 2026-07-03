# Headless 模式

cosh-core 的 headless 模式通过 stdin/stdout 提供 JSONL 协议接口，是 cosh-shell 与 cosh-core 之间的标准通信方式，也可供任意客户端集成。

## 启动

```bash
# 长驻模式（持续接收 JSONL）
cosh-core --headless

# 单条提示（执行完自动退出）
cosh-core --headless "帮我查看磁盘使用情况"

# 配合参数
cosh-core --headless --model qwen-max --approval-mode trust
```

## 输入消息（Shell → Core）

所有输入通过 stdin 逐行发送，每行一个 JSON 对象，以 `type` 字段区分消息类型。

### user — 用户消息

```json
{
  "type": "user",
  "message": { "role": "user", "content": "列出当前目录文件" },
  "session_id": "optional-session-id",
  "shell_context": { "cwd": "/home/user/project", "env": {}, "last_exit_code": 0 }
}
```

### control_request — 控制请求

```json
{
  "type": "control_request",
  "request_id": "req-001",
  "request": { "subtype": "initialize" }
}
```

支持的 `subtype`：

| subtype | 说明 |
|---------|------|
| `initialize` | 初始化会话（返回能力声明 + 系统 init 消息） |
| `interrupt` | 中断当前生成 |
| `shutdown` | 关闭进程 |
| `switch_model` | 切换模型（需 `model` 字段） |
| `reload_config` | 重新加载 config.toml |
| `config_override` | 运行时覆盖配置（`approval_mode`、`allowed_tools`） |

### control_response — 控制响应（回复 Core 的请求）

```json
{
  "type": "control_response",
  "response": {
    "subtype": "tool_approval",
    "request_id": "req-002",
    "response": { "behavior": "allow" }
  }
}
```

### registry_request — 注册表请求

```json
{
  "type": "registry_request",
  "request_id": "reg-001",
  "domain": "tools",
  "action": "list",
  "params": {}
}
```

## 输出消息（Core → Shell）

所有输出写入 stdout，同样逐行 JSON。

### system — 系统消息

```json
{"type":"system","subtype":"init","session_id":"...","model":"qwen3.7-plus","tools":["ask_user_question","edit","grep","read_file","shell","skill","todo","write_file"]}
```

常见 `subtype`：`init`、`auth_required`、`auth_ok`、`model_switched`、`config_reloaded`。

### stream_event — 流式事件

LLM 生成过程中逐 token 输出：

```json
{"type":"stream_event","event":{"subtype":"text_delta","content":"你好"}}
{"type":"stream_event","event":{"subtype":"thinking_delta","content":"让我分析..."}}
{"type":"stream_event","event":{"subtype":"tool_use_begin","tool_name":"shell","tool_use_id":"tu-1"}}
{"type":"stream_event","event":{"subtype":"tool_use_delta","content":"{\"command\":\"df -h\"}"}}
{"type":"stream_event","event":{"subtype":"tool_use_end"}}
```

### control_request — Core 发起的请求

Core 需要 Shell 协作时（如工具审批、用户提问）：

```json
{
  "type": "control_request",
  "request_id": "cr-001",
  "request": { "subtype": "can_use_tool", "tool_name": "shell", "tool_input": {"command": "rm -rf /tmp/old"} }
}
```

### result — 轮次结束

```json
{"type":"result","subtype":"success","is_error":false,"result":"completed","session_id":"...","duration_ms":1234}
```

## 初始化握手

标准启动序列：

```
Shell → Core:  {"type":"control_request","request_id":"init-1","request":{"subtype":"initialize"}}
Core → Shell:  {"type":"control_response","response":{"subtype":"success","request_id":"init-1","response":{"subtype":"initialize","capabilities":{...}}}}
Core → Shell:  {"type":"system","subtype":"init","session_id":"...","model":"...","tools":[...]}
```

`capabilities` 声明 Core 支持的协议能力：

| 能力 | 说明 |
|------|------|
| `can_handle_can_use_tool` | 支持工具审批协议 |
| `can_handle_host_executed_shell_tool_result` | 支持 Shell 端执行结果回传 |
| `can_handle_shell_evidence_tool` | 支持终端证据工具 |

## 认证流程

若未配置 API 密钥，Core 在初始化时发送认证请求：

```
Core → Shell:  {"type":"control_request","request_id":"auth-init","request":{"subtype":"auth_required","reason":"not_configured","providers":[...]}}
Shell → Core:  {"type":"control_response","response":{"subtype":"auth_response","request_id":"auth-init","response":{"provider_id":"dashscope","values":{"api_key":"sk-xxx"},"persist":true}}}
Core → Shell:  {"type":"system","subtype":"auth_ok"}
```

## 会话恢复

```bash
cosh-core --headless --resume <session-id>
```

Core 从 `~/.copilot-shell/sessions/<session-id>.json` 加载历史消息，恢复对话上下文。

## 错误处理

协议级错误通过 `result` 消息传递：

```json
{"type":"result","is_error":true,"errors":["provider returned HTTP 429: rate limit exceeded"],"session_id":"..."}
```

输入行 JSON 解析失败时静默忽略（仅 debug 日志），不会终止进程。
