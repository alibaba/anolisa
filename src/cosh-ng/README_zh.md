# cosh-ng

[English](README.md)

cosh-ng 是一个 AI 原生 Linux 终端。它以你已经在用的 Shell 为基础。
启动 `cosh` 后，bash 或 zsh 照常工作。遇到更复杂的任务，直接用自然语言请 Agent
检查或操作即可。Shell 命令、Skills、审批卡片和可恢复对话都留在同一个终端里。
需要自动化或集成其他 Agent 时，还可以使用结构化 JSON 和 JSONL 接口。

## 为什么使用 cosh-ng

| 传统终端 | cosh-ng |
|---|---|
| 需要把意图翻译成命令 | 可以直接运行命令，也可以用自然语言描述任务 |
| 自动化散落在脚本中 | 用 Skills 封装可复用工作流 |
| AI 上下文绑定在单个聊天窗口 | 按工作空间恢复 Agent 对话 |
| AI 操作难以检查 | 通过审批卡片和审计记录检查工具调用 |
| 不同发行版使用不同系统命令 | 用 `cosh-cli` 获得稳定、结构化的系统操作 |

交互程序、管道、重定向、任务控制、bash/zsh 配置和 `Ctrl+C` 都会在前台终端中
照常工作。

## 安装

使用 ANOLISA CLI 安装。

```bash
curl -fsSL https://get.agentic-os.sh | bash
sudo anolisa --install-mode system install cosh-ng
```

在 Alibaba Cloud Linux 上，也可以直接安装 RPM。

```bash
sudo yum install cosh-ng
```

以上安装方式目前面向 Linux。macOS 请按照
[开发者入门指南](../../docs/developer-guide/zh/cosh-ng/getting-started.md)从源码构建。

## 30 秒开始使用

```bash
cd your-project
cosh
```

进入后，可以在同一个会话里运行 Shell 命令，也可以直接交代 Agent 任务。

```text
$ git status
$ 分析这个服务为什么反复重启，并展示判断依据
$ /agent
$ /skills list
$ /session status
```

用 `/auth` 选择 provider，用 `/help` 查看当前版本支持的命令。如果希望每次 Agent
调用工具前都等待确认，运行 `/mode approval recommend`。Shell 和 Core 的审批设置
统一使用 `recommend`、`auto` 或 `trust`。使用 cosh-core runtime 时，`/agent`
会打开一次性 Composer，可在开头指定 `/skill:<name>`，并添加经过验证的工作空间内
`@路径`引用。

## 文档

- [用户手册](../../docs/user-guide/zh/user-entrypoint/cosh-ng/README.md)
- [接入 MCP server](../../docs/user-guide/zh/user-entrypoint/cosh-ng/mcp.md)
- [交互式终端](../../docs/user-guide/zh/user-entrypoint/cosh-ng/shell/overview.md)
- [配置](../../docs/user-guide/zh/user-entrypoint/cosh-ng/configuration.md)
- [管理系统操作](../../docs/user-guide/zh/user-entrypoint/cosh-ng/cli/overview.md)
- [Headless 集成](../../docs/user-guide/zh/user-entrypoint/cosh-ng/core/headless-mode.md)
- [开发者入门](../../docs/developer-guide/zh/cosh-ng/getting-started.md)
- [架构](../../docs/developer-guide/zh/cosh-ng/architecture.md)
- [贡献指南](CONTRIBUTING_zh.md)

## 参与贡献

源码构建主要面向贡献者，请从[开发者指南](../../docs/developer-guide/zh/cosh-ng/getting-started.md)
开始。

## 许可证

Apache-2.0
