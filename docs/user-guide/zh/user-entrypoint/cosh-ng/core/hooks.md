# 钩子系统

cosh-core 的钩子系统允许在 Agent 执行流程的关键节点注入外部逻辑。钩子通过执行外部命令实现，支持拦截、审计和上下文注入。

## 事件点

| 事件 | 触发时机 | 可拦截 |
|------|----------|--------|
| `PreToolUse` | 工具执行前 | 是（block/allow/ask） |
| `PostToolUse` | 工具执行后 | 是（block/allow） |
| `PostToolUseFailure` | 工具执行失败后 | 否 |
| `UserPromptSubmit` | 用户消息提交时 | 是（block/allow） |
| `SessionStart` | 会话初始化完成后 | 否 |
| `Stop` | Agent 决定停止时 | 是（block/allow） |
| `BeforeModel` | LLM 请求发送前 | 否 |
| `AfterModel` | LLM 响应接收后 | 否 |

## 配置

在 `~/.copilot-shell/config.toml` 中定义：

```toml
[hooks]
enabled = true

[[hooks.PreToolUse]]
name = "security-check"
command = "/usr/local/bin/my-security-hook"
timeout = 5000

[[hooks.SessionStart]]
name = "context-loader"
command = "/usr/local/bin/load-context"
timeout = 3000
```

钩子也可通过扩展的 `cosh-extension.json` 注册。

## 协议

### 输入（stdin → 钩子进程）

Core 以 JSON 格式将事件数据写入钩子进程的 stdin：

```json
{
  "session_id": "abc-123",
  "cwd": "/home/user/project",
  "hook_event_name": "PreToolUse",
  "timestamp": "2026-07-01T10:00:00Z",
  "transcript_path": "/home/user/.copilot-shell/sessions/abc-123",
  "tool_name": "shell",
  "tool_input": { "command": "rm -rf /tmp/old" }
}
```

### 输出（钩子进程 → stdout）

钩子以 JSON 格式返回决策：

```json
{
  "decision": "block",
  "reason": "危险的 rm -rf 命令",
  "systemMessage": "该命令被安全策略拦截"
}
```

### 决策值

| decision | 含义 |
|----------|------|
| `allow` | 允许继续 |
| `block` / `deny` | 拦截，终止该操作 |
| `ask` | 需要用户确认 |
| 无 / 空 | 透传（不干预） |

### 附加字段

| 字段 | 说明 |
|------|------|
| `reason` | 决策原因（block/deny 时嵌入决策，同时作为通知消息后备） |
| `systemMessage` | 通知消息（优先于 reason 展示给用户） |
| `hookSpecificOutput` | 自定义 JSON 数据（其中 `additional_context` 会注入对话上下文） |

## 执行模型

1. 同一事件点可注册多个钩子，按配置顺序依次执行
2. 任一钩子返回 `block`，则最终决策为 block（短路）
3. 钩子超时（默认 5000ms）视为透传
4. 钩子进程退出码非零视为错误，不影响主流程
5. 无 `name` 字段的钩子定义会被跳过

## 通知

钩子执行后产生的通知通过 JSONL 输出传递给 Shell：

```json
{"type":"stream_event","event":{"subtype":"hook_notification","hook_name":"security-check","message":"该命令被安全策略拦截","decision":"block"}}
```

Shell 端负责渲染通知卡片。

## 扩展钩子

通过 `cosh-extension.json` 注册的钩子与配置文件中的钩子合并，使用相同的执行协议。参见 [extensions.md](extensions.md)。
