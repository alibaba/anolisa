# cosh-ng 架构

[English](../../en/cosh-ng/architecture.md)

cosh-ng 将交互式终端、Agent 运行时、确定性的操作系统 API 和逐步形成的 Gateway control plane
分开。每个边界都能独立测试，也可以由其他程序单独集成。下文 Gateway 内容是候选工作树中的局部基础，
不是上游 production service。

## 上游系统视图

```text
bash/zsh <--- cosh-shell
                  |
                  | JSONL
                  v
              cosh-core
                  |
                  +--> provider / tools / MCP
                  |
                  +--> cosh-platform ---> cosh-types

caller ---> cosh-cli ---> cosh-platform ---> cosh-types
```

安装后的 `cosh` 启动器通常执行 `cosh-shell raw cosh-core`。`cosh-shell` 编译时不依赖工作空间中的其他 crate，运行时则维护一个长时间存活的 cosh-core 子进程。两端都可能独立失败或重启，因此 stdin/stdout 协议需要保持向后兼容。

Gateway 规划固定的上游基线是 `fa0c8369d300d90a6470965dc564e20b09487eb7`。该基线包含上图五个
crate 与 runtime path，但没有 `cosh-gateway` 或 `cosh-gateway-contracts` crate。

## 候选 Gateway 基础

基于该基线的共享候选工作树增加两个 library crate：

```text
cosh-gateway-contracts --> TaskAggregate --> SQLite Task/event/receipt/Outbox transaction
        |
        +---------------> Capability Broker slice（in-memory，targeted test）

cosh-gateway ----------> RuntimeSupervisor --> private COSH JSONL v1 codec
                                   `-------> official ACP wire-v1 codec/Bridge
                                              + 固定 installed-adapter profile

未来 CoshCoreBridge --> contract public mapping + supervisor + codec

Gateway daemon/API、CoshCoreBridge、已安装 ACP entrypoint、完整 ACP
domain/governance mapping、Shell Attachment 与 Web presentation 均未实现。
```

Task reducer 与 SQLite store 是 local control-plane 基础。Runtime supervisor 独占一个 direct child
process group、bounded stdout/stderr、escalation/reap 与一次 process terminal observation。它的
cosh-core codec 使用现有 **private COSH control protocol v1**，不是 ACP，也尚未映射为 public
Runtime event。

当前不存在 executable Gateway entry point 或 authenticated Unix/network API。Shell path 没有改变，
`cosh-shell` 仍拥有 native PTY 与 compatibility cosh-core process。候选树准确固定官方 ACP Rust SDK
2.0.0，把组件 baseline 提升到 Rust 1.88，并增加 supervised stable-v1 stdio slice 以及已安装
`codex-acp`/`claude-agent-acp` 的内置 profile。这里没有 package runner 或 network bootstrap
路径。Library 已有支持独立 cancel 的有界 Session Driver，仍缺已安装 entrypoint、production
Permission UI/evidence 与 real-adapter conformance 证据。

## Crate 职责

| Crate | 二进制 | 拥有 | 不应拥有 |
|---|---|---|---|
| `cosh-types` | 无 | 无副作用的响应、错误、配置、审计、快照协议类型 | 操作系统访问或运行策略 |
| `cosh-platform` | 无 | 发行版检测、软件包和服务适配器、审计策略与存储、ws-ckpt 客户端 | CLI 展示或 Agent 交互 |
| `cosh-cli` | `cosh-cli` | Clap 命令、JSON 响应、退出状态 | 平台适配器之外的发行版分支 |
| `cosh-core` | `cosh-core` | 模型服务、工具循环、Hooks、Skills、MCP、Extensions、注册表、会话和压缩 | 终端控制或前台 PTY 交互 |
| `cosh-shell` | `cosh-shell` | PTY 宿主、输入路由、卡片、审批、终端证据、界面、core 进程生命周期 | 模型服务实现或直接抽象操作系统 API |
| `cosh-gateway-contracts`（候选） | 无 | 无副作用的 Task、Runtime、Capability、identity、header 与 error contract，leaf string/digest 有界 | Storage、process ownership、transport、provider、OS execution 或尚未实现的 aggregate admission limit |
| `cosh-gateway`（候选） | 无 | 局部 Task reducer/SQLite store、Runtime supervision/private core codec、ACP v1 codec/Bridge 与固定 installed-adapter profile、Capability integration slice | Shell PTY、已安装 Gateway/ACP entrypoint、把 provider/ACP wire type 当作 domain contract、绕过 Broker 的 OS effect 或未治理的 ACP callback |

## 交互数据流

1. `cosh-shell` 在 PTY 中启动 bash/zsh，并安装 OSC 生命周期标记。
2. 输入路由把 Shell 语法发送给 PTY，把斜杠命令发送给本地控制入口，把自然语言发送给 Agent 适配器。
3. 默认适配器维护 cosh-core 进程，每轮 Agent 对话发送一条 JSONL 用户消息。
4. cosh-core 解析工作区配置、模型服务、Skills、Extensions、MCP 工具和会话状态，随后流式返回事件。
5. cosh-shell 治理这些事件，并渲染文本、问题卡片或审批卡片。
6. 经过审批的 Shell 命令交回前台 PTY。OSC 终端证据与 Agent 任务关联，并在 core 请求时返回。
7. Extension 重载等注册表修改复用同一个长期运行的 core，并在安全的版本边界发布。

## 确定性 CLI 数据流

```text
Clap command
  → command module 校验参数
  → cosh-platform 选择后端
  → 后端返回类型化数据或 CoshError
  → cosh-cli 输出 CoshResponse<T>
  → 成功退出 0，操作失败退出 1
```

软件包和服务写操作支持 `--dry-run`。快照调用通过 Unix socket，以 bincode 和四字节小端长度前缀进行通信。

## cosh-shell 模块职责

| 目录 | 职责 |
|---|---|
| `shell_host/` | PTY 生命周期、OSC 解析、Shell 集成、raw relay |
| `raw_input/` 和 `input/` | 终端模式、多行输入、输入 relay |
| `slash/` | 斜杠命令解析、注册和展示 |
| `adapter/` | 模型服务与 core 适配器、控制协议传输 |
| `agent/` | Agent 任务生命周期和受控事件 |
| `runtime/` | 编排、共享状态、分发和启动 |
| `approval/` 和 `question/` | 用户决策和控制响应 |
| `hooks/` | Hook 策略和执行，通过运行边界交接修改 |
| `tools/` | 命令风险模型、只读规则和工具展示 |
| `ui/` | 终端渲染和卡片组件 |
| `evidence/`、`journal/`、`ledger/` | 有范围限制的终端证据和决策记录 |

不要在 `cosh-shell/src/` 根目录新增实现文件。保持模块边界清晰，并在结构改动后运行 `crates/cosh-shell/scripts/check-layout.sh`。

## 兼容性和安全契约

- `CoshResponse<T>` 是稳定的自动化信封。
- ws-ckpt 枚举顺序属于二进制协议格式。
- cosh-core 消息使用逐行 JSON，headless 模式的 stdout 不能混入日志或界面文本。
- 正在运行的 Agent 任务固定使用启动时的注册表版本。新版本检查通过后，在空闲时立即启用，否则等待安全时机。
- 会话状态按工作区隔离。恢复只还原模型可见对话，不还原历史终端证据。
- Core 读取工具固定在启动时规范化的工作区。后续 `cd` 只改变 Shell 目录，不会移动读取边界。越过路径或挂载点时会拒绝访问。
- 前台 Shell 交接串行执行。只有内核证据表明前台进程正在等待输入时，才应用输入等待超时。管道和全屏程序不受此限制。
- Linux 包路由可使用 `ID_LIKE` 中第一个可识别家族，但 typed 和 JSON 输出仍保留发行版的真实 `ID`。
- 工具自动审批在无法判断时拒绝执行。直接匹配原始命令子串不能充当安全边界。

## Gateway 与 ACP 交付边界

候选 library 尚未组成持久 production Gateway，仍缺 Gateway API/daemon、Task coordinator 与
lease/recovery loop、完整 Capability enforcement、集成 CoshCore Bridge、已安装 ACP Runtime
entrypoint/Session Driver、production Permission Proxy、real-adapter 证据、Shell Attachment 与
Web/channel presentation。[ACP v1 Phase 0-2 规划集](../../../../src/cosh-ng/docs/design/acp-v1-phase-0-2/README_zh.md)
区分固定的上游基线与候选实现证据，并定义剩余模块边界、Warp 对比、交付顺序与验收 Gate。
Phase 0-2 总体状态仍为 **NOT ACCEPTED**。

继续阅读[开发 cosh-ng](getting-started.md)、[IPC 协议](ipc-protocol.md)和[测试](testing.md)。
