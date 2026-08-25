# cosh-ng 用户手册

[English](../../../en/user-entrypoint/cosh-ng/README.md)

cosh-ng 是一个 AI 原生 Linux 终端，默认使用 Enhanced Assisted，也提供显式的
无 Hook Native 集成。先阅读快速开始，再按下面的任务导航查找所需功能或命令。

## 从这里开始

- [快速开始](QUICKSTART.md)：安装 cosh-ng 并完成第一个任务。
- [模型提供商](core/providers.md)：配置认证并选择模型提供商。
- [配置](configuration.md)：了解配置文件、设置项和优先级。
- [支持的平台](supported-distros.md)：确认软件包和服务后端。

## 在终端工作

| 目标 | 继续阅读 |
|---|---|
| 在同一会话中使用 Shell 命令和自然语言任务 | [交互式终端](shell/overview.md) |
| 选择 Agent 工具调用何时需要确认 | [工具审批](shell/approval.md) |
| 恢复或压缩会话 | [会话恢复](shell/session-recovery.md) |
| 了解斜杠命令和按键行为 | [交互行为](shell/interactive-mode.md) |

## 添加可复用能力

| 目标 | 继续阅读 |
|---|---|
| 在项目或团队之间共享操作说明 | [Skills](core/skills.md) |
| 接入本地进程或远程服务提供的工具 | [接入 MCP 服务](mcp.md) |
| 打包 Skills、Hooks、设置和工具 | [Extensions](core/extensions.md) |
| 在 Agent 生命周期事件前后运行检查 | [Hooks](core/hooks.md) |

## 管理系统操作

先运行只读命令。对支持的包管理或服务变更先加 `--dry-run` 预览；这类操作通常需要 root 权限。

| 目标 | 继续阅读 |
|---|---|
| 查找、安装或删除软件包 | [软件包管理](cli/package-management.md) |
| 查看或修改 systemd 服务 | [服务管理](cli/service-management.md) |
| 使用现有的 `cosh-cli` 工作区快照命令 | [工作区快照](cli/checkpoint.md) |
| 查看策略决策和审计事件 | [安全审计](cli/audit.md) |

工作区快照页面描述现有的 `cosh-cli` system-operations 路径。它与 task-only
Gateway profile 独立；package Gateway 不依赖 `ws-ckpt`，也不暴露 checkpoint operation。

## 集成与自动化

`cosh agent` launcher 由 ANOLISA 与 RPM package 安装。源码构建与 unified build 只安装
Gateway binary；此时请替换为 `cosh-gateway doctor`、`cosh-gateway run` 或
`cosh-gateway task`，其余参数保持不变。

- 运行 `cosh agent doctor --profile codex --workspace "$PWD"` 检查单独安装的
  `codex-acp`，也可以选择 `claude-code` profile 检查 `claude-agent-acp`。把有界 UTF-8
  prompt 通过管道传给 `cosh agent run` 即可执行一轮任务；增加 `--output jsonl` 可以获得
  稳定的流式事件。COSH 不运行 `npx`、不下载 package，也不接受任意 Adapter command。
  Permission request 使用 `/dev/tty`，stdin 只传递 prompt。默认的
  `--permission prompt` 只提供 `allow_once` 与 `reject_once`；没有 TTY、只有不支持的
  choice、遇到 EOF 或使用 `--permission deny` 时都取消且不授权。脱敏 append-only
  evidence 默认写入 `$XDG_STATE_HOME/cosh/gateway/permission-evidence.jsonl`，没有设置
  `XDG_STATE_HOME` 时使用
  `$HOME/.local/state/cosh/gateway/permission-evidence.jsonl`。可以用绝对路径
  `--permission-evidence PATH` 覆盖。COSH 只存储 digest 与 decision class，不保存 raw
  prompt、tool argument、option label、session identifier 或 workspace path。Evidence
  持久化失败时，callback 会被取消且本轮运行失败。这两个 direct ACP command 不受 durable
  Gateway Task Plane 治理，适合本地 interoperability。
- 对于 durable local Task，使用 package 安装的 system-scope
  `cosh-gateway@.service` unit。它选择 contained `core` runtime 和
  `gateway-brokered-v1` profile，并接纳配置的 canonical workspace：

  Unit 默认把 Core `HOME` 设为 private systemd `StateDirectory` 下的
  `/var/lib/cosh-gateway-%i/core-home`。Provider config 可以放在
  `/var/lib/cosh-gateway-$USER/core-home/.copilot-shell/config.toml`，也可以使用
  `/etc/copilot-shell/config.toml` system config。不要在
  `/etc/cosh/gateway-$USER.env` 中把 `HOME` 设到该 `StateDirectory` 之外。
  `EnvironmentFile` 中的值会覆盖 unit 的安全默认值，而 admitted workspace 与其他 host
  path 对这个 contained Core profile 保持只读。

  ```bash
  sudo install -d -m 0755 /etc/cosh
  sudo install -m 0600 /dev/null "/etc/cosh/gateway-$USER.env"
  printf '%s\n' \
    "COSH_GATEWAY_WORKSPACE=$PWD" | \
    sudo tee "/etc/cosh/gateway-$USER.env" >/dev/null
  sudo systemctl start "cosh-gateway@$USER.service"
  gateway_socket="/run/cosh-gateway-$USER/gateway.sock"
  ```

  Unit 会传入 `--systemd-unit`；Gateway 在 bind socket 前校验 live cgroup membership、
  control-group kill、最终 `SIGKILL`、main-process exit tracking 与 disabled delegation。
  Direct `serve` 没有该 proof 时会 fail closed。Service 还会向 Runtime descendant 隐藏
  per-user service-manager socket。启动过程会 canonicalize workspace，把 admitted target 固定为
  `workspace/cosh/task-only-v1`，把 Runtime selector 固定为 `core`/`gateway-brokered-v1`。
  Daemon 会把每个 Unix peer 认证为 local OS actor；如果 submission 的 target 或 selector 不同，
  会在 Task 创建前被拒绝。
- 在另一个 Terminal 把 `gateway_socket` 设为相同绝对路径，再把 intent 传给 Task API：

  ```bash
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

  Task API 支持 `submit`、`get`、`events`、`append`、`cancel`、`retry` 和
  `resolve-approval`。`append` 用来回答 profile 的 durable `ask_user_question` request。
  `resolve-approval` 仍属于通用 API，但这个 profile 没有需要 approval 的 side effect，因此
  不会产生 approval flow。Idempotency key 让客户端在 I/O 不确定后可以安全重试；durable Task、
  Runtime 和 Outbox state 支持查看、取消和显式 retry，不会重放未知的 side effect。
- Task-only profile 有意不暴露 checkpoint、write、Shell、slash command、Web、channel 或
  remote capability。交互式 slash command 仍由 `cosh-shell` 负责，不是 Gateway Task command。
  `SIGINT` 与 `SIGTERM` 会触发有界的 scheduler 与 Runtime shutdown，Gateway 只监听本地 Unix
  socket。仓库自动执行 Fake Adapter conformance；真实 Codex/Claude Adapter 检查与人工 Terminal
  验收仍是独立的、与具体安装相关的 gate。
- [结构化 OS CLI](cli/overview.md)：命令域和安全的自动化方式。
- [输出格式](output-format.md)：`CoshResponse<T>` 成功和失败响应封装。
- [无界面模式](core/headless-mode.md)：供其他前端使用的 JSONL 集成。
- [Agent 工具](core/tools.md)：工具边界和审批行为。
