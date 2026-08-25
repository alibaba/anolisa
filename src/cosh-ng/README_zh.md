# cosh-ng

[English](README.md)

cosh-ng 是一个以现有 Shell 为基础的 AI 原生终端。`cosh` 默认使用 Enhanced
Assisted 模式，保留隐式自然语言路由、Skills、审批卡片和可恢复 Agent 对话。
如果要求 bash 或 zsh 独占会话，不加载 Cosh Hook、不观察也不提供洞察，可以
在启动时选择 Native 集成。自动化或其他 Agent 集成仍可使用结构化 JSON 和
JSONL 接口。

## 为什么使用 cosh-ng

| 传统终端 | cosh-ng |
|---|---|
| 需要把意图翻译成命令 | 默认 Assisted 模式可混合自然语言和命令 |
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

Enhanced Assisted 是默认模式。`◇ ` 前缀表示 Cosh 可能在前台 Shell 执行前
分类并路由本次输入。

```text
◇ user@host:~/project$ git status
◇ user@host:~/project$ 分析这个服务为什么反复重启
```

在空提示符按 `Shift+Tab` 可切换到 Enhanced Shell-only。`◌ ` 前缀表示普通
输入交给 Shell，但仍可获得命令执行后的洞察。再次按下即可返回 Assisted。

如果会话要求完全不加载 Cosh Hook、不观察也不提供洞察，可显式启动 Native。

```bash
COSH_SHELL_INTEGRATION=native cosh
```

```text
$ hello
bash: hello: command not found
```

用 `/auth` 选择 provider，用 `/help` 查看当前版本支持的命令。如果希望每次 Agent
调用工具前都等待确认，运行 `/mode approval recommend`。Shell 和 Core 的审批设置
统一使用 `recommend`、`auto` 或 `trust`。增强集成使用 cosh-core runtime 时，`/agent`
会打开一次性 Composer，可在开头指定 `/skill:<name>`，并添加经过验证的工作空间内
`@路径`引用。

如果要在不进入交互式 Shell 的情况下运行本机已安装的 ACP Adapter，可以先检查
Adapter，再通过 stdin 发送 prompt。

下面的命令使用 ANOLISA 或 RPM 安装的 `cosh agent` launcher。源码构建或 unified build
只安装 Gateway binary，此时请使用 `cosh-gateway doctor`、`cosh-gateway run` 或
`cosh-gateway task`，其余参数保持不变。

```bash
cosh agent doctor --profile codex --workspace "$PWD"
printf '%s\n' 'summarize the current changes' | \
  cosh agent run --profile codex --workspace "$PWD"
```

首个版本只接受内置 `codex` 与 `claude-code` profile。对应的 `codex-acp` 或
`claude-agent-acp` executable 需要单独安装。COSH 在 runtime 中不会调用 `npx`，也不会
下载 Adapter。Permission callback 只在本地 controlling terminal 上提示；没有 TTY 或使用
`--permission deny` 时，COSH 会取消请求。Once-only decision 会以脱敏 evidence 形式记录到
private local state directory。

Package Gateway 提供一个受 containment 保护的本地 Task Plane。它只在 package 安装的
systemd service 中调度 Task；即使 Gateway hard crash，该 service 仍负责完整 Runtime
cgroup。`gateway-brokered-v1` Core profile 有意保持为 task-only：Runtime inventory
只有无副作用的 `ask_user_question` capability。该 profile 不提供 checkpoint、write、Shell、
slash command、Web 或 remote capability，也没有需要 approval 的 side effect。

配置 workspace 并启动按 account 命名的 Gateway instance。

Core unit 默认把 `HOME` 设为 private systemd `StateDirectory` 下的
`/var/lib/cosh-gateway-%i/core-home`。Core provider config 可以放在
`/var/lib/cosh-gateway-$USER/core-home/.copilot-shell/config.toml`，也可以使用
`/etc/copilot-shell/config.toml` system config。不要在
`/etc/cosh/gateway-$USER.env` 中把 `HOME` 覆盖到该 `StateDirectory` 之外；environment
file 的优先级高于安全默认值，而 admitted workspace 与其他 host path 在这个 unit 中是只读的。

```bash
sudo install -d -m 0755 /etc/cosh
sudo install -m 0600 /dev/null "/etc/cosh/gateway-$USER.env"
printf '%s\n' \
  "COSH_GATEWAY_WORKSPACE=$PWD" | \
  sudo tee "/etc/cosh/gateway-$USER.env" >/dev/null
sudo systemctl start "cosh-gateway@$USER.service"
gateway_socket="/run/cosh-gateway-$USER/gateway.sock"
printf '%s\n' 'inspect the failed service' | \
  cosh agent task --socket "$gateway_socket" submit \
    --runtime core --runtime-profile gateway-brokered-v1 \
    --idempotency-key '<stable-submit-key>'
cosh agent task --socket "$gateway_socket" get '<tsk_UUID>'
cosh agent task --socket "$gateway_socket" events '<tsk_UUID>' --after 0 --limit 64
printf '%s\n' 'answer to the question' | \
  cosh agent task --socket "$gateway_socket" append '<tsk_UUID>' \
    --input-request-id '<inp_UUID>' --idempotency-key '<stable-input-key>'
cosh agent task --socket "$gateway_socket" cancel '<tsk_UUID>' --run-id '<run_UUID>' \
  --idempotency-key '<stable-cancel-key>'
cosh agent task --socket "$gateway_socket" retry '<tsk_UUID>' \
  --previous-run-id '<run_UUID>' --idempotency-key '<stable-retry-key>'
```

Daemon 首次启动时会生成并持久化 installation ID，也可以通过 `--installation-id` 显式 provision。
请把示例中的 typed identifier 替换成 Task API 返回的值。Task API 支持 `submit`、`get`、
`events`、`append`、`cancel`、`retry` 和 `resolve-approval`；`append` 用来回答 profile
产生的 durable user question，而这个 profile 不会产生 approval request。
Direct `serve` 没有 package unit 的 live `--systemd-unit` proof 时会 fail closed；Gateway 会在
创建 socket 或 database 前完成校验。Daemon 会把 Unix peer 认证为 local OS actor，将 target
固定为 `workspace/cosh/task-only-v1`，只接受 `core`/`gateway-brokered-v1` selector 与配置的
canonical workspace，持久化 Runtime binding，并由 scheduler 投递 durable Outbox work。
本地非托管 ACP interoperability 应使用 `doctor` 与 `run`，不能使用 `serve`；这两个 direct
ACP command 不受 durable Task Plane 治理。
Task Plane 不依赖 checkpoint 或 ws-ckpt。现有的 `cosh-cli checkpoint` 命令仍是独立的
system-operations 路径，不会为这个 Gateway profile 增加 checkpoint capability。

`SIGINT` 与 `SIGTERM` 会在 Daemon 退出前触发有界的 scheduler 与 Runtime shutdown。Daemon
仍然只监听 Unix socket，不开放 remote listener。

仓库为 direct ACP path 提供 Fake Adapter conformance coverage。具体安装在投入生产前，仍需
另行执行真实 Codex/Claude Adapter 检查与人工 Terminal 验收。

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

## 数据采集

cosh-ng 会采集匿名的运行指标用于改进服务质量，包括工具调用次数、token 用量、
审批统计、操作系统类型/架构，以及一个持久的安装 UUID 用于跨会话
关联。**不采集用户输入内容、代码内容或对话内容。**

关闭当前用户的遥测：

```bash
mkdir -p ~/.copilot-shell
touch ~/.copilot-shell/telemetry_disabled
```

系统管理员也可以通过创建系统级哨兵文件，为整台机器上的所有用户关闭遥测：

```bash
sudo mkdir -p /etc/anolisa
sudo touch /etc/anolisa/.telemetry_disabled
```

## 参与贡献

源码构建主要面向贡献者，请从[开发者指南](../../docs/developer-guide/zh/cosh-ng/getting-started.md)
开始。

## 许可证

Apache-2.0
