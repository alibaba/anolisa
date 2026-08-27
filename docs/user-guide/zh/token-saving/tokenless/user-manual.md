# Tokenless 用户手册

[English](../../../en/token-saving/tokenless/user-manual.md)

Tokenless 面向工具调用密集的 AI Agent。它的 CLI 可以精简 Schema 和 JSON 响应，Adapter 还可以改写 Shell 命令、检查工具依赖，并把压缩结果交给 Agent。最终效果取决于宿主框架：有的 Adapter 会替换原始结果，有的只会追加压缩上下文而保留原文。

第一次使用请从[快速开始](QUICKSTART.md)进入。

## 从源码构建独立 CLI

源码构建适合开发和调试。当前项目只在 Linux 上验证和支持源码构建：

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa/src/tokenless
cargo build --release --locked -p tokenless-cli
./target/release/tokenless --version
```

这条路径只生成独立的 `tokenless` CLI，不会安装 `rtk` 或 Agent 接入资源。需要在 Agent 中使用完整能力时，请按照[快速开始](QUICKSTART.md)通过 anolisa CLI 安装。

## 从源码构建 Python SDK

CPython 应用可以在进程内使用 Tokenless，无需为每个生命周期操作启动 CLI：

```bash
make python-wheel
python3 -m venv /tmp/tokenless-python
/tmp/tokenless-python/bin/pip install target/wheels/anolisa_tokenless-*.whl
```

构建要求系统可发现 CPython 3.11+ 开发环境。Wheel 使用 CPython 3.11 stable ABI，但仍与
构建时的操作系统和 CPU 架构绑定。

Python SDK 分为两层。`anolisa-tokenless` 包开放通用 `TokenlessSdk`、可直接调用的
`TokenlessRuntime` 操作和 typed `TokenlessStats` 查询；相同版本的
`anolisa-tokenless-agentscope` 包把该通用生命周期映射到 AgentScope。两层结构、可运行示例与
配置见 [Python SDK 指南](sdk.md)。

## 能力与边界

| 能力 | 当前代码实际执行的行为 | 重要边界 |
|------|------------------------|----------|
| Schema 压缩 | 移除 `title` 和 `examples`，删除描述中的围栏代码和行内代码，合并空白并截断描述 | 通过 cosh/Cosh-NG 的 `BeforeModel` Hook 和 OpenCode 的逐工具定义 Hook 运行（Qwen Code 清单中的条目会被当前宿主跳过）；其他场景可直接调用 CLI |
| Content-aware 响应压缩 | 把 JSON Records、构建日志和长纯文本路由到匹配 Compressor，只接受端到端更小的结果 | 直接 `compress-response` 仍只处理 JSON；宿主能力决定共享 Hook 能否用文本替换并发布恢复能力 |
| TOON 编码 | 编码 JSON；估算 Token 没有下降时保留 JSON 输入 | 宿主支持文本替换时替换原文；无替换能力的宿主透传 |
| 命令重写 | 有匹配规则时调用 `rtk rewrite`，再向框架提交改写后的 Shell 输入 | 真正提交给 Shell 的命令会变化；无规则或被拒绝时透传 |
| Tool Ready | 旧版调用前能力，用于检查声明的二进制、版本、配置、权限和可选依赖 | 已硬关闭；不会检查、修复或阻断工具调用 |
| Stash | 保存因字符串、数组、深度或 Schema 描述截断以及 build/log 间隙移除而省略的内容 | 默认 TTL 一小时、最多 10,000 个有效条目；其他被移除字段不会进入 Stash |

代码没有提供固定节省率保证。结果取决于 Payload、Adapter 交付语义，以及工具数据在模型上下文中的占比。请按[效果度量](measuring-savings.md)使用自己的工作负载测量。

## Tokenless 如何参与一次工具调用

启用对应 Adapter 后，一次工具调用可能经过以下阶段：

```text
工具调用前：已硬关闭的 Tool Ready Hook → 命令重写
工具调用后：内容检测 → 符合条件的 Compressor → 可选 Stash/TOON → 写入统计
模型调用前：Schema 压缩
```

这是能力示意，不是所有框架都会完整执行的固定流水线。例如 content-aware Protocol 路径
当前服务于 Cosh-NG、Qoder、受支持的 Claude Code 版本和 OpenCode；OpenClaw、Hermes 与
DeepSeek Harness 保留专用 JSON 响应路径。Codex 和 Qwen Code 当前宿主契约不能替换工具后
输出。具体见 [Agent 集成](framework-integration.md)。

## 需要特别理解的行为

### 安装不等于启用

`anolisa install tokenless` 安装组件和 Adapter 资源。要让某个 Agent 自动使用 Tokenless，还需要：

```bash
anolisa adapter enable tokenless <framework>
```

CLI-only 用法不需要 Adapter。

### “关闭压缩”只影响压缩操作

设置 `compression_enabled=false` 或 `TOKENLESS_COMPRESSION_ENABLED=0` 后，`compress`、
`compress-schema`、`compress-response` 和 `compress-toon` 仍会计算预测节省并可能写入统计，
但会返回原始输入。该模式不会写入 Stash 条目。

这个设置不会关闭 RTK 命令重写、Adapter 执行或内容取回。Tool Ready 已独立硬关闭。如需停止 Agent 中的所有 Tokenless 行为，应禁用 Adapter：

```bash
anolisa adapter disable tokenless <framework>
```

### 可逆压缩是有条件的

启用压缩时，响应/Schema 截断和 build/log 省略区间默认会把被移除的 Payload 写入
`~/.tokenless/stash.db`，并在输出中加入：

```text
<<tokenless:0123456789abcdef01234567>>
```

可以通过 `tokenless retrieve` 或 MCP `tokenless_retrieve` 取回。以下情况会失去可逆性：

- 使用了 `--no-stash`。
- 压缩处于 dry-run 模式。
- Stash 数据库不可用或写入失败。
- 条目已经超过 TTL。
- 有效条目超过 10,000 个后，较早条目被容量策略淘汰。
- 调用方使用了不同的 Stash 数据库路径。

Stash 并不能让所有压缩都可逆。被移除的 `debug`/`trace` 字段、`null` 和空值、Schema `title`/`examples` 以及 Markdown 格式不会保存供取回。启用实际压缩前，应使用有代表性的数据验证关键 Payload。

### 普通处理错误通常 fail-open

缺少 `tokenless` 或 `rtk`、压缩无收益或发生普通处理错误时，压缩和重写 Hook 通常不返回
修改。`compress` 命令的所有非 `applied` 响应都会返回原文。Tool Ready 会在旧版检查、修复和
阻断逻辑之前硬退出；工具执行后的失败归因是独立能力，保持不变。

命令重写也会改变宿主提交的 Shell 命令。大多数 Adapter 会直接替换命令输入；Hermes 会先阻止第一次调用，再提示 Agent 使用改写命令重试。因此，除了压缩结果，还应验证重要命令工作流。

## 支持的 Agent Adapter

| Agent 产品 | 集成方式 | 当前代码路径 |
|------|----------|--------------|
| cosh | Extension | Tool Ready（已硬关闭）、命令重写、Schema；Cosh-NG 替换符合条件的 Pipeline 输出，旧版 Copilot Shell 透传工具后输出 |
| OpenClaw | Plugin | Tool Ready（已硬关闭）、`exec` 命令重写、替换持久化结果、可选 TOON；无 Schema |
| Hermes | Plugin | Tool Ready（已硬关闭）、阻止后重试的命令重写、用响应压缩 + TOON 替换结果；无 Schema |
| Qoder | Plugin | Tool Ready（已硬关闭）、命令重写、通过 `updatedToolOutput` 交付响应 Pipeline；无 Schema |
| Claude Code | Marketplace Plugin | Tool Ready（已硬关闭）、Bash 命令重写；Claude Code 2.1.121 及以上可替换响应；条件式 TOON；无 Schema |
| Codex | Plugin | Tool Ready（已硬关闭）、RTK 命令重写、环境失败诊断；不替换响应/TOON，无 Schema |
| OpenCode | Plugin | Tool Ready（已硬关闭）、Bash 命令重写、用响应压缩 + TOON 替换工具输出、Schema |
| Qwen Code | Extension | Tool Ready（已硬关闭）、命令重写；当前宿主缺少工具后替换能力，并跳过声明的 BeforeModel 事件 |

## 支持的 Agent 开发框架

| 框架 | 集成方式 | 当前代码路径 |
|------|----------|--------------|
| AgentScope | 进程内 Python Middleware | 通过独立 Python 包替换成功的最终工具响应，并提供受 marker 约束的恢复 Tool |

## 按任务查找文档

| 我想做什么 | 文档 |
|------------|------|
| 第一次安装并验证 | [快速开始](QUICKSTART.md) |
| 从源码构建独立 CLI | [本页 · 从源码构建独立 CLI](#从源码构建独立-cli) |
| 使用进程内 Python SDK | [Python SDK](sdk.md) |
| 集成 AgentScope | [AgentScope SDK 集成](sdk/agentscope.md) |
| 接入 Agent 产品 | [Agent 集成](framework-integration.md) |
| 手动压缩、取回或运行 MCP | [CLI 参考](cli-reference.md) |
| 查看节省或内容变化、做双跑对比 | [效果度量](measuring-savings.md) |
| 修改配置或了解本地数据 | [配置与数据隐私](configuration-and-privacy.md) |
| 解决无统计、Adapter 或 Stash 问题 | [故障排查](troubleshooting.md) |
| 排查 Schema 压缩没有记录 | [故障排查 · Schema 压缩没有统计记录](troubleshooting.md#schema-压缩没有统计记录) |
| 升级或卸载 | [故障排查 · 升级与卸载](troubleshooting.md#升级与卸载) |

## 推荐的上线顺序

1. 在非敏感测试任务中完成[快速开始](QUICKSTART.md)。
2. 使用 dry-run 记录同一任务的基线。
3. 开启真实压缩并比较结果质量与节省。
4. 确认本地数据和 SLS 策略符合要求。
5. 再为生产使用的 Agent 启用 Adapter。

Tokenless 的配置和 CLI 以当前安装版本的 `tokenless --help` 为最终依据。
