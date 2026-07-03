# cosh-core 总览

cosh-core 是 cosh-ng 的 AI Agent 运行时核心。它提供无头 JSONL 后端，集成 LLM 提供商、钩子系统、工具执行、技能管理和会话持久化。

## 定位

cosh-core 是 cosh-shell 的后端引擎。cosh-shell 通过 stdin/stdout 与 cosh-core 进程通信（JSONL 协议）。cosh-core 也可以独立使用：

- **单条提示模式** — 直接传入 prompt，执行完退出
- **Headless 模式** — 长驻进程，持续接收 JSONL 消息
- **Registry 模式** — 仅处理注册表请求后退出

## 运行模式

```bash
# 单条提示（需显式 --headless）
cosh-core --headless "帮我查看系统负载"

# 长驻 headless 模式（通过 stdin 接收 JSONL）
cosh-core --headless

# Registry 模式
cosh-core --registry

# 覆盖模型
cosh-core --headless --model qwen-max "分析这段代码"

# 覆盖审批模式
cosh-core --headless --approval-mode trust "安装 nginx"

# 恢复会话
cosh-core --headless --resume <session-id>
```

## CLI 参数

| 参数 | 说明 |
|------|------|
| `--headless` | 强制 headless JSONL 模式 |
| `--model <name>` | 覆盖配置中的模型 |
| `--approval-mode <mode>` | 覆盖审批模式（trust/auto/balanced/strict） |
| `--allowed-tools <tools>` | 逗号分隔的自动审批工具列表 |
| `--resume <session-id>` | 恢复已有会话 |
| `--verbose` | 增加日志详细程度 |
| `--registry` | Registry 模式 |
| `--enable-shell-evidence-tool` | 启用终端输出证据工具 |

## 核心能力

| 能力 | 说明 | 详细文档 |
|------|------|----------|
| LLM 提供商 | OpenAI 兼容 / Aliyun SysOM | [providers.md](providers.md) |
| 钩子系统 | 8 个事件点，可扩展 | [hooks.md](hooks.md) |
| 工具执行 | 内置工具 + 自定义工具 | [tools.md](tools.md) |
| 技能管理 | Markdown 技能定义 | [skills.md](skills.md) |
| 扩展加载 | cosh-extension.json | [extensions.md](extensions.md) |
| 会话持久化 | JSON 格式会话存储 | — |

## 架构概览

```
stdin (JSONL)                stdout (JSONL)
     │                            ▲
     ▼                            │
┌────────────────────────────────────────┐
│              cosh-core                 │
│  ┌──────┐  ┌──────────┐  ┌──────────┐  │
│  │ Auth │  │ Provider │  │  Tools   │  │
│  └──────┘  └──────────┘  └──────────┘  │
│  ┌──────┐  ┌──────────┐  ┌──────────┐  │
│  │Hooks │  │ Session  │  │  Skills  │  │
│  └──────┘  └──────────┘  └──────────┘  │
└────────────────────────────────────────┘
```

## 认证流程

若未配置 API 密钥，cosh-core 在启动时发送 `auth_required` 控制请求：

1. Core 发送 `AuthRequired`，列出可用认证提供商
2. Shell（或外部客户端）展示认证 UI
3. 用户选择提供商并填写凭证
4. Shell 回送 `ControlResponse` 包含凭证
5. Core 应用凭证，可选持久化到 config.toml
6. Core 发送 `auth_ok` 状态，开始正常工作
