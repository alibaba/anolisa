# Token-Less

[English](README.md)

LLM Token 优化工具包——Schema/响应压缩 + 命令重写 + 工具环境就绪检查。Token-Less 是 [ANOLISA](../../README_zh.md) 的 Token 节省组件，通过多种互补策略最小化 LLM Token 消耗。

## 核心能力

| 能力 | Token 节省 | 说明 |
|------|-----------|------|
| Schema 压缩 | ~57% | 压缩 OpenAI Function Calling 工具定义 |
| 响应压缩 | ~26–78% | 压缩 API/工具响应（因内容类型而异） |
| TOON 上下文压缩 | 15–40% | 将 JSON 编码为 TOON 格式 |
| 命令重写 | 60–90% | 通过 RTK 过滤 CLI 输出（支持 70+ 命令） |
| Tool Ready | 减少重试浪费 | 旧版调用前预检、自动修复与阻断；当前硬关闭 |

Tool Ready 当前在所有 Adapter 中无条件硬旁路，不会读取依赖规范、执行调用前检查、自动修复环境或阻止工具调用。任何环境变量都无法恢复旧行为；重新启用必须修改源码并重新发布。

工具执行后的失败归因、响应压缩、RTK 命令重写、TOON、Stash 和统计是独立能力，仍保持原有行为。

## 适用场景与预期效果

tokenless 只优化**工具调用响应**进入 LLM 上下文前的冗余，不触及模型推理与对话历史。收益高度取决于会话中工具响应的占比与形态。

### 哪些场景收益高

| 工作负载 | 主要受益策略 | 原因 |
|----------|-------------|------|
| Shell 密集（编译/测试/排查） | 命令重写（RTK） | `cargo`/`npm`/`go`/`pytest` 等输出含大量进度/警告噪声，RTK 削减 60–90% |
| API/抓取密集（REST、web_fetch） | 响应压缩 + TOON | JSON 含 debug/null/空值与语法开销，压缩 26–78%，TOON 再省 15–40% |
| 工具数量多的 Agent | Schema 压缩 | 大量 Function Calling 定义冗余描述，~57% |
| 长响应需保真 | 可逆压缩（Stash） | 截断后可 `retrieve` 原文，端到端无损，可放心收紧阈值 |

### 哪些场景收益低或不适用

- **纯对话/少工具调用**：工具响应占比极低，整体节省接近 0。
- **没有固定最小 Payload**：`compress-schema` 和 `compress-response` 会为每个通过输入
  规则的合法 JSON 生成候选结果。在 Active 模式下，只有候选结果的估算 Token 数严格少于原文时
  才输出它。包含可移除内容的小输入仍可能被压缩，而已经紧凑的较大输入也可能原样透传；CLI 会把
  原因写入 stderr，且不记录统计。在 Dry-run 模式下，CLI 始终输出原文，并可能把较小的候选结果
  记为预测节省。[CLI 参考](../../docs/user-guide/zh/token-saving/tokenless/cli-reference.md)
  中的描述、字符串、数组和深度阈值只决定单项转换何时触发，并不是整个 Payload 的最小大小；
  Agent Adapter 还可能应用独立的
  [预检门槛](../../docs/user-guide/zh/token-saving/tokenless/framework-integration.md#adapter-处理规则)。
- **模型推理 token / 计费 token**：不在 tokenless 经手范围。

### 预期效果估算

> 下表比例为**示意性经验估值**，随任务差异很大，非实测常数。

| 会话组成 | 典型占比 | tokenless 能否优化 |
|----------|---------|-------------------|
| LLM 推理输出（文本生成） | ~35% | ❌ 不涉及 |
| LLM 输入（system prompt + 对话历史） | ~40% | ❌ 不涉及 |
| 工具调用参数 | ~5% | ❌ 不涉及 |
| **工具响应（API 返回 + 命令输出）** | **~20%** | **✅ 优化范围** |

**实际节省率 = 面板节省率 × 工具响应占比**

例如：面板显示压缩率 60%，若工具响应占总消耗 20%，实际节省率为 60% × 20% = **12%**。这也是为何在总消耗 1500 万 Token 的实验中节省量观感偏小——tokenless 只作用于其中约 300 万 Token 的工具响应部分。

> Stash 使压缩**端到端无损**：可适度收紧截断阈值换取更高 inline 节省，需要原文时经 `<<tokenless:KEY>>` 标记取回，不影响正确性。建议用 `TOKENLESS_COMPRESSION_ENABLED=0/1` 双跑对照真实节省。
> 各策略触发条件与阈值见 [用户手册](../../docs/user-guide/zh/token-saving/tokenless/user-manual.md)。

## 集成路径

### Agent Adapter

- **OpenClaw 插件** — 命令重写 + 响应压缩 + Schema 压缩
- **copilot-shell 钩子** — Tool Ready（已硬关闭）+ 命令重写 + 响应压缩 + TOON
- **Hermes Agent 插件** — Tool Ready（已硬关闭）+ 命令重写 + 响应压缩 + TOON
- **Qoder CLI 插件** — Tool Ready（已硬关闭）+ 命令重写 + 响应压缩
- **Claude Code 插件** — Tool Ready（已硬关闭）+ 命令重写 + 响应压缩 + TOON
- **Codex 插件** — Tool Ready（已硬关闭）+ 命令重写 + 响应压缩 + TOON
- **OpenCode 插件** — Tool Ready（已硬关闭）+ 命令重写 + Schema/响应压缩 + TOON
- **DeepSeek Harness 插件**。通过 DSH 原生 `tools/post-execute` 接入响应压缩和环境错误归因

### Agent 开发框架集成

- **AgentScope Python 集成** — 完整开放 Schema 压缩、RTK 改写、响应压缩、TOON、
  受 marker 约束的恢复和归属统计。

## 快速开始

首选 ANOLISA CLI 安装已发布的组件。

安装脚本会把 `anolisa` 放到 `~/.local/bin`。user mode 安装的 `tokenless`
和 `rtk` 也在这个目录。如果当前 Shell 还找不到命令，先把该目录加入
`PATH`。

```bash
curl -fsSL https://get.agentic-os.sh | bash

# 让默认安装目录在当前 Shell 中生效
export PATH="$HOME/.local/bin:$PATH"
anolisa --version
anolisa install tokenless
tokenless --version
```

已配置 YUM 源的 Alinux 用户也可以安装 RPM 包。

```bash
sudo yum install anolisa tokenless
sudo anolisa --install-mode system adopt tokenless
```

从同一 YUM 源安装 CLI 后，`sudo` 可以从系统路径找到 `anolisa`。`adopt` 会把
直接安装的 RPM 写入 system 状态，adapter 命令随后才能读取组件契约。

当前公开软件包支持 Linux x86_64、aarch64 和 macOS Apple Silicon。Intel
Mac 暂无已发布的软件包。仓库中的 npm packaging 目录用于构建发布产物，
目前不能通过公开的 `anolisa-tokenless` npm 包安装。源码中保留的
`@anolisa/tokenless-darwin-x64` optional dependency 只是发布构建目标，
不代表 registry 中已有可安装的软件包。

通过 ANOLISA 管理的安装或已执行 `adopt` 的 RPM 会放置可用 adapter，但不会
直接改动 Agent 产品的用户配置。请用拥有该配置的用户执行以下命令，并且只启用
准备使用的 adapter。

```bash
anolisa adapter scan
anolisa adapter enable tokenless openclaw
anolisa adapter status tokenless
```

DeepSeek Harness 必须指定至少一个 profile。需要启用多个 profile 时，应在同一条
命令中列出全部名称，完整集合语义见下文。启动 DSH 时请使用已经启用的名称。

```bash
anolisa adapter enable tokenless dsh --profile <profile>
dsh --profile <profile>
```

从源码构建适合开发者。

```bash
git clone <repo-url>
cd Token-Less

# 完整安装，构建并安装二进制，随后部署所有 adapter
make setup
```

源码安装会把 `tokenless` 放在 `~/.local/bin`，`rtk` 辅助
二进制也位于同一个目录，并部署开发所需的全部 adapter。

### 构建 Python Runtime

框架开发者可以从源码构建进程内 Python API：

```bash
make python-wheel
python3 -m venv /tmp/tokenless-python
/tmp/tokenless-python/bin/pip install target/wheels/anolisa_tokenless-*.whl
```

该目标要求系统可发现 CPython 3.11+ 开发环境，并默认通过 `uvx` 提供
Maturin。请先安装 [`uv`](https://docs.astral.sh/uv/)，或者在 `PATH` 中已有
兼容 Maturin 时执行 `make python-wheel MATURIN=maturin`。执行
`cargo test --workspace` 同样需要该 Python 环境；普通 Cargo workspace 默认
命令不包含 Python Extension。

`anolisa_tokenless` 模块支持 CPython 3.11 及更高版本，但只能在构建该原生
Wheel 的对应平台使用。它开放四个 Tokenless 生命周期接口并内置对应平台的 RTK；
TOON 已链接进原生 Runtime，不依赖 Tokenless CLI 或系统 helper。仓库会构建并测试该包，
但目前尚未发布到
PyPI。具体见 [Runtime 设计](docs/design/runtime-library_zh.md) 和
[用户手册](../../docs/user-guide/zh/token-saving/tokenless/user-manual.md#从源码构建-python-runtime)。

同一个 Wheel 还提供不依赖 CLI 的只读 typed Stats 查询。可以让 `TokenlessStats` 指向
Runtime 使用的状态目录，或使用延迟创建的 `sdk.stats`：

```python
from anolisa_tokenless import TokenlessStats

stats = TokenlessStats("/absolute/path/to/tokenless-data")
summary = stats.summary()
print(summary.total.tokens_saved, summary.total.tokens_saved_percent)
```

Token 数量是估算值，并且只有产生正向节省的操作才会记录。`show()` 和详细 `diff()`
结果可能包含 `stats.db` 中保存的敏感工具输入与输出。这里的只读是指 API 能力；客户端
打开时遵循 CLI 初始化流程，可能创建或迁移 `stats.db`，因此数据目录必须可写。
`summary(limit=None)` 和 `compare(..., limit=None)` 最多查询最近 10,000 条记录；Session
或 Tool-use Diff 最多读取最近 10,000 条匹配记录。要获得有意义的对比，应先传入 dry-run
Session，再传入启用 Tokenless 的 Session。

### OpenCode 安装

OpenCode 适配器通过 `tool.execute.before/after` 原生插件事件注册已硬关闭的 Tool Ready、
RTK 命令重写和响应/TOON 压缩，并通过 `tool.definition` 压缩工具 Schema。
压缩后的响应会替换原始模型可见输出，避免重复占用上下文。

```bash
make opencode-install
```

安装器会在 OpenCode 全局 `plugins/` 目录中创建 `tokenless.js` 符号链接，
不会覆盖同名的非托管文件。配置目录支持 `OPENCODE_CONFIG_DIR`、
`XDG_CONFIG_HOME` 和显式的 `TOKENLESS_OPENCODE_CONFIG_DIR` 覆盖。
安装后重启 OpenCode 即可加载插件。

### DeepSeek Harness 插件

DSH 原生 Bundle 通过 `tools/post-execute` 压缩成功的单文本块 JSON 工具结果。
Tokenless CLI 只有返回更短的合法 JSON 时才会替换结果，内容读取类工具默认保持
原样。关闭响应压缩、跳过压缩或压缩无收益时，环境错误归因仍会工作。

需要启用多个 DSH profile 时，应在同一条命令中重复传入 `--profile`。

```bash
anolisa adapter enable tokenless dsh \
  --profile web \
  --profile headless
```

每次 enable 或 re-enable 都会把本次传入的 profile 视为完整目标集合。旧 receipt
中已有但本次没有列出的 profile 会卸载 Bundle，因此每次都要列出需要继续使用
Tokenless 的全部 profile。每个名称必须与 `dsh --profile <profile>` 使用的名称
一致。配置写在对应 profile 的 `cordis.patch.yml` 中。全部配置项和默认值见
[DeepSeek Harness 集成参考](../../docs/user-guide/zh/token-saving/tokenless/framework-integration.md#deepseek-harness-原生处理路径)。

### AgentScope 框架集成

AgentScope 1.0.11 至 1.0.x 及 AgentScope 2.0.x 应用需要显式安装两个相同版本的 Python
Wheel。框架集成直接调用 `anolisa-tokenless` Runtime，不会启动 CLI 子进程。两个 Python
包当前都尚未发布到包索引。当前应从源码 checkout 构建并同时安装两个 Wheel：

```bash
make python-wheel agentscope-wheel
python -m pip install \
  target/wheels/anolisa_tokenless-*.whl \
  target/wheels/anolisa_tokenless_agentscope-*.whl
```

两个大版本使用相同的公开入口和配置对象；由于 AgentScope 1.x 与 2.x 提供的生命周期
扩展点不同，仅最后的挂载方式不同。

AgentScope 1.x 使用 Tokenless Toolkit，因此在 Agent 构造前后动态注册的普通工具和
MCP 工具都会获得相同的生命周期处理。安装时必须显式提供 Session 标识。

```python
from agentscope.agent import ReActAgent
from tokenless_agentscope import TokenlessAgentScope, TokenlessConfig

integration = TokenlessAgentScope(
    TokenlessConfig(
        mode="balanced",
        data_dir="/absolute/path/to/tenant-tokenless-data",
    ),
)
toolkit = integration.create_toolkit()
toolkit.register_tool_function(application_tool)
agent = ReActAgent(..., toolkit=toolkit)
integration.install(agent, session_id="conversation-id")
```

AgentScope 2.x 在构造阶段接收恢复 Tool 和中间件；该方式从 2.0.0 即可使用，不依赖后续
补丁版本才新增的 Toolkit 动态修改接口。

```python
from agentscope.agent import Agent
from agentscope.tool import Toolkit
from tokenless_agentscope import TokenlessAgentScope, TokenlessConfig

integration = TokenlessAgentScope(
    TokenlessConfig(
        mode="balanced",
        data_dir="/absolute/path/to/tenant-tokenless-data",
        # retrieve_tool_name="tenant_tokenless_retrieve",
    ),
)
toolkit = Toolkit(tools=[*application_tools, *integration.tools])

agent = Agent(
    ...,
    toolkit=toolkit,
    middlewares=integration.middlewares,
)
```

AgentScope App 从 2.0.1 开始支持。它会在配置的绝对基础目录下，为每个
user/agent/session 派生独立的 Tokenless 数据目录：

```python
from agentscope.app import create_app

app = create_app(..., **integration.app_options())
```

如果应用已经定义 `tokenless_retrieve`，应在 `TokenlessConfig` 中设置唯一的
`retrieve_tool_name`；App 组装阶段不会把其他工具暴露给该 factory，无法预先检查重名。

AgentScope 2.0.0 尚未提供 App 级 Agent middleware 和 Tool 注入，因此该补丁版本只支持
直接构造 Agent。原有 `TokenlessMiddleware` 2.x API 继续保留兼容；新代码应使用
`TokenlessAgentScope`，避免依赖特定补丁版本的 Toolkit 动态修改或 Tool 自动收集行为。

| 模式 | 策略 |
|---|---|
| `conservative` | 所有未排除工具使用 1 MiB / 65,536 / 深度 32 限制 |
| `balanced` | 跳过 Read/Glob/Grep；Shell 使用 65,536 / 128 / 深度 8，其他采用 conservative 限制 |
| `aggressive` | 跳过 Read/Glob/Grep；其他采用 CLI 默认的 4,096 / 32 / 深度 8 |

默认模式为 `balanced`。只有模型当前可见 marker 时才会向模型发布只读恢复 Tool，
并且它只接受本次模型调用保留的精确 marker 集合中的 hash。直接构造 Agent 时，每个
用户或租户必须显式传入不同的绝对 `data_dir`；省略 `data_dir` 时，
`TOKENLESS_DATA_DIR` 只作为进程级回退。除非应用有明确生命周期策略，否则保留默认
一小时 stash TTL，且不要依赖跨节点恢复。

两个 AgentScope Adapter 都启用 Schema 压缩、RTK 命令改写、响应压缩、TOON、恢复、
环境错误提示和逐调用归属。原生 Wheel 内置 RTK 并直接链接 TOON，不搜索系统可执行文件。
宿主对象和流式 chunk 保持不变，只转换复制后的调用参数和最终模型可见文本。Tool Ready
仍保持硬关闭。

## Raw 打包

Raw 打包接收同一目录中已经构建好的 `tokenless` 和 `rtk`，并按照
组件维护的稳定目录结构生成制品：

```bash
make package-raw \
  BIN_DIR="$PWD/target/release-bins" \
  TARGET_OS=linux \
  TARGET_ARCH=aarch64 \
  OUTPUT_DIR="$PWD/dist"
```

Raw 支持矩阵为 `linux-x86_64`、`linux-aarch64` 和 `macos-aarch64`。
输入可使用 `darwin`/`arm64`、`amd64`/`x64` 别名，产物名始终采用 ANOLISA
规范名称。脚本不会执行跨平台二进制，而是直接检查 ELF 或 Mach-O 架构，
并负责嵌入组件自维护的 `.anolisa/component.toml`、展开适配器 Hook 符号链接、
统一权限以及生成可复现的
`tokenless-<version>-<os>-<arch>.tar.gz`。需要固定其他时间戳时可传入
`SOURCE_DATE_EPOCH`。

npm 打包同样从 `target/npm-prebuilt` 下读取预构建的 `linux-x64`、
`linux-arm64`、`darwin-x64`、`darwin-arm64` 四个二进制目录，并负责校验和组装：

```bash
node npm/scripts/package-npm.js --all
```

固定目录结构和单目标接口见 [npm/README.md](npm/README.md#packaging-for-npm)。

## 查看 Token 节省明细

`show` 用于原样打印完整的压缩前后内容；`diff` 用于解释估算 Token
节省，并只突出发生变化的行：

```bash
tokenless stats show 42
tokenless stats diff 42
tokenless stats diff --session <session-id>
tokenless stats diff --session <session-id> --tool-use-id <tool-use-id>
tokenless stats diff 42 --json
```

Session 总览只包含指标；单记录和 tool-use 报告包含 unified content
diff。只有相邻 active 阶段的输出与输入内容完全一致时才会串成一条链，
从而避免重复计算中间阶段的 Token。完整选项和度量限制见
[Tokenless 效果度量](../../docs/user-guide/zh/token-saving/tokenless/measuring-savings.md)。

## 数据库位置

Tokenless 默认将统计数据和可逆压缩数据分别存储在
`~/.tokenless/stats.db` 与 `~/.tokenless/stash.db`。可为两个数据库统一
指定目录：

```bash
export TOKENLESS_DATA_DIR="$HOME/path/to/tokenless-data"
```

该目录可以是当前用户有权访问的任意绝对路径，包括 `/var/lib` 下由服务管理
的目录；文件系统根目录、相对路径和父目录遍历会被拒绝。若只需自定义一个
数据库，现有的 `TOKENLESS_STATS_DB`、`TOKENLESS_STASH_DB` 和 `--stash-db`
覆盖项优先级更高，但必须位于真实用户 home 或选定的数据目录下。配置文件
仍位于 `~/.tokenless/config.json`。

## Tool Ready

旧版 Tool Ready 会在工具调用前预检 `tool-ready-spec.json` 中声明的环境依赖，
缺失时报告 `NOT_READY` 并提示跳过重试。当前已无条件硬关闭，Hook 会在读取规范、
检查、修复或阻断之前返回；工具执行后的失败归因保持独立。

```bash
# 报告单个工具对应的硬关闭状态
tokenless env-check --tool Shell

# 报告全部工具模式的硬关闭状态
tokenless env-check --all

# 报告清单模式的硬关闭状态
tokenless env-check --checklist

# 机器可读的硬关闭状态；不会输出 tools/summary 清单
tokenless env-check --checklist --json

# 为兼容性保留；不会检查或修复环境
tokenless env-check --tool Shell --fix
```

这些命令当前只报告 Tool Ready 已硬关闭，不会检查或修改环境。
所有 JSON 模式都只返回相同的三个字段：

```json
{"tool":"checklist","status":"UNKNOWN","enabled":false}
```

`tool` 表示指定的工具或 `all`/`checklist` 范围。硬旁路生效期间绝不会输出
休眠旧版实现的 `tools` 与 `summary` 清单字段。

## 架构

- `crates/tokenless-schema/` — 核心库：SchemaCompressor + ResponseCompressor
- `crates/tokenless-ccr/` — 可逆压缩缓存（Compress-Cache-Retrieve）
- `crates/tokenless-runtime/` — CLI 与语言绑定共用的有状态 Rust API
- `crates/tokenless-cli/` — CLI 二进制
- `python/tokenless/` — 面向 CPython 3.11+ 的 PyO3 `anolisa_tokenless` 包
- `python/agentscope/` — 独立的 AgentScope 框架集成与 Wheel 元数据
- `adapters/tokenless/` — 面向具体 Agent/CLI 的 Plugin、Hook 与 Extension 适配器包
- `adapters/tokenless/dsh/`。DeepSeek Harness 原生 Bundle
- `third_party/rtk/` — RTK 命令重写引擎（vendored）
- `packaging/raw/` — Tokenless 自维护的 ANOLISA Raw 打包与目标校验

## 前置依赖

- **Rust** toolchain >= 1.89 — RTK（edition 2024）及 toon-format 所需
- **just** — 用于下载并应用 RTK Patch
- **Git** — 用于通过 justfile 下载 RTK 源码
- **CPython 3.11+ 开发环境与 uv** — 仅构建 Python Wheel 或显式包含全部
  workspace member 的命令需要

## 许可证

Apache License 2.0 — 详见 [LICENSE](../../LICENSE)。
