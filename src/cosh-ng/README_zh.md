# cosh-ng

[English](README.md)

cosh-ng 是一个 AI 原生终端。它以你已经在用的 Shell 为基础。
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

在 Alibaba Cloud Linux 4 上，通过 ANOLISA CLI 和 RPM backend 把 cosh-ng
安装到 system 范围。

```bash
curl -fsSL https://get.agentic-os.sh | bash
export PATH="$HOME/.local/bin:$PATH"
sudo "$HOME/.local/bin/anolisa" --install-mode system install cosh-ng --backend rpm
```

公共安装脚本可以合并上述步骤。

```bash
curl -fsSL https://get.agentic-os.sh | bash -s -- --cosh-ng --backend rpm --install-mode system
export PATH="$HOME/.local/bin:$PATH"
```

后续升级或卸载也使用同一个入口。

```bash
curl -fsSL https://get.agentic-os.sh | bash -s -- --cosh-ng --install-mode system --upgrade
curl -fsSL https://get.agentic-os.sh | bash -s -- --cosh-ng --install-mode system --uninstall
```

在 macOS arm64 上改用 user 范围：

```bash
curl -fsSL https://get.agentic-os.sh | bash -s -- --cosh-ng --backend raw --install-mode user
export PATH="$HOME/.local/bin:$PATH"
```

在 Alibaba Cloud Linux 4 上，也可以直接安装 RPM。

```bash
sudo yum install cosh-ng
```

当前发布的 Linux raw 契约无法覆盖所有已路由的发行版，因此不作为推荐的
Linux 安装路径。raw 包支持 macOS arm64，但依赖 Linux 的软件包和服务操作
不可用。源码构建仅供贡献者使用，请参阅
[开发者入门指南](../../docs/developer-guide/zh/cosh-ng/getting-started.md)。

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

如果要在不进入交互式 Shell 的情况下运行本机已安装的 ACP Adapter，可以先检查
Adapter，再通过 stdin 发送 prompt。

```bash
cosh agent doctor --profile codex --workspace "$PWD"
printf '%s\n' 'summarize the current changes' | \
  cosh agent run --profile codex --workspace "$PWD"
```

首个版本只接受内置 `codex` 与 `claude-code` profile。对应的 `codex-acp` 或
`claude-agent-acp` executable 需要单独安装。COSH 在 runtime 中不会调用 `npx`，也不会
下载 Adapter。这个 entrypoint 默认拒绝 permission callback。

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
