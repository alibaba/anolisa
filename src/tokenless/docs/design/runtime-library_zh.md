# Tokenless Runtime 库

[English](runtime-library.md)

## 目标

框架集成需要在进程内完成响应压缩和 Stash 取回，避免每个工具结果都启动一次
`tokenless` 子进程。Runtime 库提供这一能力，并让 CLI 与语言绑定复用同一套压缩实现。

本设计新增两个公开层次：

- `tokenless-runtime`：负责状态、策略和归因的可复用 Rust crate。
- `anolisa-tokenless`：通过 PyO3 暴露 Runtime 的原生 Python 包。

首版 Python 接口覆盖 JSON 响应压缩与 Stash 取回，并提供框架无关的
`TokenlessConfig` 和 `ToolResponseCompressor`，使不同框架复用策略、类型保真、节省检查
和 marker 约束恢复，同时保留各自的生命周期代码。Schema 压缩、TOON 编码、RTK 命令
改写、MCP 和框架专用 Middleware 不属于该库。独立的 `tokenless_agentscope` 包使用这个
API，并负责 AgentScope 生命周期契约。

## 架构

```text
tokenless-schema ─┐
tokenless-ccr ────┼──> tokenless-runtime ──> tokenless CLI
tokenless-stats ──┘              └─────────> PyO3 ──> anolisa_tokenless
                                                       └──> tokenless_agentscope
```

`tokenless-runtime` 是高层应用 API。它只打开一次 Stash 和统计数据库，每次调用执行
一次策略决策，并把调用方提供的 Agent、Session 和工具调用标识写入统计。CLI 把响应
压缩和取回委托给相同函数，因此 Python 包不会复制压缩算法。

Python 扩展面向 CPython 3.11 stable ABI 构建。Wheel 仍然与操作系统和 CPU 架构相关，
但同一平台的一个 Wheel 可以供 CPython 3.11 及更高版本使用。压缩和取回期间会释放
Python GIL。共享的 SQLite 状态沿用现有的同步 Stash 与统计实现，因此同一个 Runtime
实例可以处理并发工具调用，无需全局归因变量。

## Runtime 契约

构造函数可以接收显式数据目录。未提供时，Runtime 依次使用 `TOKENLESS_DATA_DIR` 和
passwd 中的用户主目录，并遵循与 CLI 相同的路径策略。Stash marker 可以访问对应目录
中的数据，因此每个用户或租户都应使用独立目录。

`compress_response` 接收 JSON 字符串，返回包含以下字段的结构化结果：

- 调用方应使用的输出，以及计算得到的压缩候选结果；
- `applied`、`dry-run`、`no-savings` 或 `reversibility-unavailable` 状态；
- 估算 Token 数、Stash 写入指标，以及无法生成可取回 marker 的截断次数。

Python 绑定的 `require_reversible` 默认为 `True`。请求使用 Stash 但数据库无法打开或
写入，或配置阈值无法容纳取回 marker 时，Runtime 返回原始响应，并报告
`reversibility-unavailable`。这种 fail-open 行为避免宿主框架在不知情时接受无法
恢复的截断结果。CLI 则保留现有语义：Stash
失败后可以输出有损候选结果，并沿用原有告警。

非法 JSON 和非法状态路径会返回显式错误。没有节省或可逆存储失败属于策略结果，
不会抛出异常。取回接口接受 24 位十六进制哈希，或包含 Tokenless marker 的字符串，
并原样返回存储的 UTF-8 Payload。

## 打包与验证

`make python-wheel` 使用 Maturin 把 `anolisa-tokenless` 构建到 `target/wheels/`。自定义
Cargo `python-release` profile 使用 unwind panic 语义，因为 Rust panic 不应终止嵌入
它的解释器。`make test-python-runtime` 会在全新虚拟环境中安装 Wheel，并验证压缩、
Unicode 原样取回、错误映射、并发调用和逐调用统计归因。

`python/agentscope/` 是独立的纯 Python Distribution，支持 AgentScope 1.0.11 至 1.0.x
以及 AgentScope 2.0.x。稳定入口 `TokenlessAgentScope` 会选择两个生命周期后端之一：
AgentScope 1.x 串接 Toolkit postprocessor，并把恢复绑定到 Agent memory；AgentScope 2.x
在 Agent 构造阶段提供 middleware 和显式恢复 Tool。AgentScope 2.0.0 支持直接构造
Agent；其 App 尚无 Agent middleware 或 Tool 注入能力，因此 App 集成从 2.0.1 开始。

`make agentscope-wheel` 会把它构建到同一个 `target/wheels/` 输出目录；
`make test-agentscope-integration` 会配合同版本的原生 Runtime Wheel，对 1.0.11、最新
1.0.x、2.0.0、2.0.1 App 边界、2.0.3 Tool ABI 边界和最新 2.0.x 验证压缩及 byte-exact
恢复。

本仓库负责构建和测试两个 Python Distribution，但不会把它们发布到 PyPI。正式发布
还需要发布流水线为每个受支持平台构建 Wheel，按发布策略签名或生成证明，再使用发布
凭据上传。

## 兼容性与演进

Rust API 和 Python 包从 alpha 接口开始。运行在兼容 Python 进程内的新框架集成
应依赖 Python API，而不是调用 CLI。现有 CLI 和 Hook 集成仍受支持，无需迁移。

AgentScope 包负责流式 Block 保真、生命周期挂载和模型可见状态提取。Python Runtime 包
负责公共压缩模式、工具策略、类型/节省检查和 marker 授权。该边界把补丁版本差异限制在
框架包内，并让未来框架集成复用同一公共策略。
