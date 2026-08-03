# Hook 系统

[English](../../../../en/user-entrypoint/cosh-ng/core/hooks.md)

Hooks 可以在 Agent 执行的关键位置运行外部命令，用于拦截操作、记录审计信息或补充
上下文。Hook 命令拥有实际执行能力，只应启用来源可信的配置。

## 查看和管理 Hooks

运行 `/hooks --help` 查看当前版本支持的命令。常用操作如下。

| 命令 | 用途 |
|---|---|
| `/hooks` | 查看 Shell 和 Agent Hooks 的来源、状态与项目信任状态 |
| `/hooks history`、`/hooks events` | 查看最近发现和展示事件 |
| `/hooks details <id>`、`/hooks analyze <id>`、`/hooks ignore <id>` | 处理一项发现 |
| `/hooks feedback noisy\|useful <id>` | 记录一项发现是否有用 |
| `/hooks mute <target>`、`/hooks unmute <target>` | 静音或恢复某个主题或 Hook ID |
| `/hooks enable <id>`、`/hooks disable <id>` | 修改 Hook 状态 |
| `/hooks trust-project`、`/hooks untrust-project` | 保存或撤销对项目 Hooks 的信任 |
| `/hooks clear-feedback`、`/hooks clear-project-trust` | 清除已保存的反馈或项目信任 |

项目根目录受信任前，项目 Hooks 不会执行。通过默认适配器启用 Agent Hook 后，状态会
保存到注册表。Shell Hook 的状态只在当前会话中生效。

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

在 `~/.copilot-shell/config.toml` 中定义 Hooks。

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

Core 会把 JSON 事件逐条写入 Hook 进程的 stdin。

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

Hook 通过 stdout 返回 JSON 决策。

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

### 使用 BeforeModel 改写工具声明

`BeforeModel` 输入会在 `llm_request.config.tools` 中携带本次请求的完整工具声明。
Hook 可以通过同一路径返回改写后的数组，例如缩短 `description` 与 JSON Schema。

```json
{
  "hookSpecificOutput": {
    "llm_request": {
      "config": {
        "tools": [
          { "name": "shell", "description": "压缩后的说明", "parameters": { "type": "object" } }
        ]
      }
    }
  }
}
```

改写需要满足以下约束。

- 改写只作用于当前一次 LLM 请求，工具注册表与下一轮声明不受影响
- 数组的工具数量、顺序和名称必须与输入完全一致，`parameters` 必须是 JSON object；
  否则整个数组被丢弃并回退原始声明（工具过滤不属于该协议）
- 改写只能**缩短**声明。估算 Token 数超过原声明的数组即使结构合法也会被拒绝。
  本轮的上下文预算在钩子执行前就按原声明计算完毕，更大的数组会让运行时低估真实
  请求，可能导致 provider 上下文溢出
- 多个钩子按配置顺序生效，取最后一个合法数组；非法值不会覆盖此前的合法值
- 工具声明子树豁免基于键名的脱敏，因此名为 `api_key`、`token` 的 schema 属性不会被破坏

## 执行模型

1. 同一事件点可以注册多个 Hook，系统按配置顺序执行
2. 任一 Hook 返回 `block` 后，系统立即停止并拒绝操作
3. Hook 超时默认按 5000 ms 计算，超时后允许主流程继续
4. Hook 进程以非零状态退出时记为错误，不影响主流程
5. 没有 `name` 字段的 Hook 定义会被跳过

## 通知

Hook 执行后产生的通知通过 JSONL 传给 Shell。

```json
{"type":"stream_event","event":{"subtype":"hook_notification","hook_name":"security-check","message":"该命令被安全策略拦截","decision":"block"}}
```

Shell 端负责渲染通知卡片。

## 环境变量

Hook 定义可以声明 `env`，这些变量只会注入对应的子进程。

```json
{
  "type": "command",
  "name": "tokenless-compress-schema",
  "command": "python3 ${extensionPath}/hooks/compress_schema.py",
  "env": { "TOKENLESS_AGENT_ID": "cosh-ng" }
}
```

- 子进程默认继承宿主环境，`env` 中的同名变量覆盖继承值
- 宿主进程自身永不被修改（不调用 `setenv`）
- 变量名须符合 POSIX 规则 `[A-Za-z_][A-Za-z0-9_]*`。使用 `schemaVersion: 1` 的
  Extension 清单遇到无效变量名时，会以 `extension_hook_env_name_invalid` 拒绝安装。
  配置文件与旧版清单中的 Hooks 会在启动子进程时再次校验，丢弃无效条目，
  钩子照常执行，且只记录变量名（绝不记录取值）
- `${extensionPath}` / `${workspacePath}` 只在取值中替换，变量名按字面量处理
- 宿主在声明的 env map 之后注入 `COSH_RUNTIME=cosh-ng` 与 `COSH_NG_VERSION`，
  因此其优先级高于同名 `env` 条目，钩子可据此识别运行时（用于统计归因等）。
  这些值只用于协作，不能充当安全边界。同一个清单也控制 `command`，可以在自己的
  Shell 中重新赋值或清除变量，安全逻辑不得依赖它们
- `env` 属于可执行能力，会计入 Extension 的能力指纹。新增或修改 `env` 会触发重新确认

## 扩展钩子

通过 `cosh-extension.json` 注册的钩子与配置文件中的钩子合并，使用相同的执行协议。参见 [extensions.md](extensions.md)。
