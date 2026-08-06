# Tokenless 快速开始

[English](../../../en/token-saving/tokenless/QUICKSTART.md)

## 1. Tokenless 能做什么

Tokenless 帮助 AI Agent 用更少的 Token 完成原来的工作。

开启后，你不需要改变 Prompt，也不需要改变使用 Agent 的方式，Tokenless 会在后台自动工作。

你可能会感受到：

- 冗长的中间结果占用更少 Token。
- 长任务可以为真正有用的信息留出更多空间。
- Agent 在决定下一步时，面对的信息更加简洁。

不同任务的节省效果不同。较短或以对话为主的任务可能变化不明显，请使用自己的工作负载按[查看效果](#4-查看效果)进行确认。

## 2. 安装 Tokenless

先安装 anolisa CLI，再用它安装 Tokenless：

```bash
curl -fsSL https://get.agentic-os.sh | bash
export PATH="$HOME/.local/bin:$PATH"
anolisa --version
anolisa install tokenless
tokenless --version
```

如果 `anolisa --version` 能正常返回，可以直接从 `anolisa install tokenless`
开始。上面的 PATH 设置会让默认安装目录在当前 Shell 中立即生效，新的登录 Shell
可能已经包含 `~/.local/bin`。

## 3. 开始使用 Tokenless

### 3.1 在 Agent 中使用

Tokenless 可以用于：

| Agent | 命令中使用的值 |
|-------|----------------|
| cosh / Copilot Shell | `cosh` |
| OpenClaw | `openclaw` |
| Hermes | `hermes` |
| Qoder | `qoder` |
| Claude Code | `claude-code` |
| Codex | `codex` |
| Qwen Code | `qwencode` |

先查找 Agent，再为它开启 Tokenless：

```bash
anolisa adapter scan
anolisa adapter enable tokenless <agent>
anolisa adapter status tokenless
```

开启 Tokenless 后，重启对应的 Agent CLI、IDE 或 Gateway。

#### 3.1.1 示例：OpenClaw

为 OpenClaw 开启 Tokenless，然后重启 OpenClaw Gateway：

```bash
anolisa adapter enable tokenless openclaw
anolisa adapter status tokenless
```

接着让 OpenClaw 完成一个正常任务：

> 运行当前仓库的完整测试，只总结失败项。

Prompt 中不需要提到 Tokenless。

如果 OpenClaw 在安全检查时拒绝安装，请按照 [OpenClaw 说明](framework-integration.md#openclaw)确认后再重试。

### 3.2 单独使用 CLI

可以直接体验一次响应压缩：

```bash
printf '%s\n' \
  '{"status":"ok","data":{"name":"demo","items":[1,2,3]},"debug":{"trace":"verbose"},"metadata":null}' \
  | tokenless compress-response
```

命令返回的仍是合法 JSON，其中 `debug`、`metadata` 等可移除字段会被省略。
如果输出与输入完全相同，说明当前输入没有可压缩内容；请改用包含 `debug`、`null` 或长字符串的 JSON 重试。

## 4. 查看效果

在 Agent 中使用一次 Shell、API 或其他受支持的工具后，运行：

```bash
tokenless stats list --limit 5
tokenless stats summary
```

- `stats list` 显示最近被 Tokenless 变短的结果。需要查看某次结果时，从这里复制 record ID。
- `stats summary` 显示 Tokenless 处理前后的 Token 估算值和总节省量。

对于上面的 OpenClaw 示例，可以找到包含 `openclaw` 的记录，并确认其中的 Token 数从左到右变小。

查看某条记录具体改变了什么：

```bash
tokenless stats diff <record-id>
```

如果没有记录，可能是内容没有经过 Tokenless，或处理后没有变短。请参阅[开启后没有产生统计记录](troubleshooting.md#启用后没有产生统计记录)。

Token 数只是在 Tokenless 已处理内容范围内的估算值，不等于模型账单的直接变化。统计和 diff 可能包含原始工具内容；涉及敏感数据时不要分享输出。完整说明见[效果度量](measuring-savings.md)和[配置与数据隐私](configuration-and-privacy.md)。

## 5. 平台适配性

| 平台 | anolisa CLI 安装 |
|------|------------------|
| Linux x86_64/aarch64 | 支持 |
| macOS Apple Silicon | 支持 |
| macOS x86_64 | 暂不支持 |
| Windows 或使用 musl 的 Linux（例如 Alpine） | 暂不支持 |

本页只提供 anolisa CLI 的安装路径。需要从源码构建独立 CLI 时，请参阅[用户手册 · 从源码构建独立 CLI](user-manual.md#从源码构建独立-cli)。

## 6. 下一步

- [用户手册](user-manual.md)：能力边界和文档导航
- [框架集成](framework-integration.md)：各 Agent 的启用、验证与禁用
- [CLI 参考](cli-reference.md)：全部子命令和参数
- [效果度量](measuring-savings.md)：统计、双跑对比和 AgentSight/SLS
- [配置与数据隐私](configuration-and-privacy.md)：开关、存储和敏感数据
- [故障排查](troubleshooting.md)：常见错误、升级和卸载
