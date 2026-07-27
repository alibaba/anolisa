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
{"type":"system","subtype":"init","session_id":"...","session_resumable":true,"model":"qwen3.7-plus","tools":["ask_user_question","edit","grep","read_file","shell","skill","todo","write_file"]}
```

常见 `subtype`：`init`、`auth_required`、`auth_ok`、`model_switched`、`config_reloaded`。
`init` 会携带 `session_resumable`；当其为 `false` 时，调用方不得复用输出的
会话 ID。

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
Core → Shell:  {"type":"system","subtype":"init","session_id":"...","session_resumable":true,"model":"...","tools":[...]}
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

Core 会解析 `--workspace` 指定的工作空间（未提供时使用进程 cwd），加载该工作
空间的 `.copilot-shell/config.toml`、验证规范 UUID，并加载与 cosh-shell
交互式恢复共用的版本化会话信封。默认根目录为
`~/.copilot-shell/cosh-core/sessions/`，记录位于确定性的工作空间作用域目录下。
相对 `session.persist_dir` 也从该工作空间解析，而不是从无关的启动器 cwd 解析。

传入升级前原始消息数组文件的 UUID 时，Core 只检查能够证明归请求工作空间
所有的旧版扁平目录。对原先的相对默认值，该路径是
`<workspace>/sessions/<uuid>.json`；作用域不明确的共享根目录和启动器 cwd
不会被搜索。Core 只在内存中加载旧数组而不改写它；只有后续成功持久化才会
原子写入 schema v1 信封，然后删除旧文件。旧文件不含工作空间身份，因此不会
出现在选择器摘要中。能够证明归当前工作空间所有的旧版 ID 仍会纳入全部清理；
显式清理也会删除旧版文件或其升级后的副本。

provider 会话 ID 在启动后不可变。后续用户消息缺少 ID 或携带旧版
`"default"` 值时不会覆盖该身份；不同的显式 ID 会作为可恢复协议错误被拒绝。
只要本轮已经修改模型可见历史，Core 就会持久化，包括最终遇到可恢复 provider
错误的轮次。

新会话会在认证和 provider 选择完成后确定并存储模型。恢复会话通常沿用历史
模型；显式 `--model <name>` 的优先级高于历史值。

当 `session.auto_persist = false` 时，Core 只在当前进程内保留历史，并输出
`session_resumable: false`。cosh-shell 不会提交该 UUID，也不会在后续调用中
把它传给 `--resume`。

交互流程和存储行为详见[会话恢复](../shell/session-recovery.md)。

## 错误处理

协议级错误通过 `result` 消息传递：

```json
{"type":"result","is_error":true,"errors":["provider returned HTTP 429: rate limit exceeded"],"session_id":"..."}
```

会话加载与持久化失败还会携带独立于 provider 错误文本的机器可读错误码和阶段：

```json
{"type":"result","is_error":true,"errors":["session recovery failed [not_found]: session not found"],"session_error_code":"not_found","session_error_phase":"load","session_id":"..."}
```

持久化失败使用 `"session_error_phase":"persist"`。自动续接无法持久化时，
cosh-shell 只释放匹配的 active 会话，避免静默重试陈旧历史；无关的选择保持
不变。

输入行 JSON 解析失败时静默忽略（仅 debug 日志），不会终止进程。
