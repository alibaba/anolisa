# Tokenless Agent 与框架集成

[English](../../../en/token-saving/tokenless/framework-integration.md)

Tokenless 提供两类集成。Agent Adapter 通过 Plugin、Hook 和 Extension，把已安装的
二进制接入具体 Agent 产品；AgentScope 支持则是供应用开发者显式安装和注册的进程内
Python 框架包。

## Agent Adapter 支持矩阵

| Agent 产品 | 值 | Tool Ready | 命令重写行为 | 响应交付方式 | TOON | Schema |
|------|----|------------|--------------|--------------|------|--------|
| cosh | `cosh` | 已硬关闭 | 替换受支持的 Shell 输入 | Cosh-NG 替换响应；旧版 Copilot Shell 追加上下文 | 在响应压缩后尝试 | ✅ |
| OpenClaw | `openclaw` | 已硬关闭 | 替换 `exec` 命令输入 | 替换持久化工具结果消息 | 默认关闭，需主动启用 | — |
| Hermes | `hermes` | 已硬关闭 | 阻止第一次调用并要求 Agent 重试 | 替换结果字符串 | 在响应压缩后尝试 | — |
| Qoder | `qoder` | 已硬关闭 | 输出改写后的 Shell 输入 | 输出 `additionalContext` | 在响应压缩后尝试 | — |
| Claude Code | `claude-code` | 已硬关闭 | 替换 Bash 输入 | 2.1.121 及以上替换输出；否则透传 | 仅在替换结果可保持文本时使用 | — |
| Codex | `codex` | 已硬关闭 | 替换受支持的 Shell 输入 | 保留原文，追加分析或压缩备选内容 | 用于生成该备选内容 | — |
| DeepSeek Harness | `dsh` | 未注册 | 未注册 | 只在结果更小时替换已接受的单文本块 JSON 结果 | 未注册 | 未注册 |
| OpenCode | `opencode` | 已硬关闭 | 替换 Bash 输入 | 替换工具输出 | 在响应压缩后尝试 | ✅ |
| Qwen Code | `qwencode` | 已硬关闭 | 输出改写后的 Shell 输入 | 输出 `additionalContext` | 在响应压缩后尝试 | ✅ |

“—”表示当前 Adapter 没有注册此能力；对应的 Tokenless CLI 命令仍可能可用。

这些 Adapter 仍会注册 Tool Ready，但会在检查、修复或阻断之前无条件硬退出，任何运行时设置都无法重新启用。工具执行后的失败归因不受影响。

`additionalContext` 是追加型 Hook 字段。在这些路径上，Tokenless 源码本身不会删除原始结果，最终处理方式还取决于宿主实现。统计记录只能证明压缩候选内容变小了，不能证明宿主已经从模型请求中移除原文。

OpenCode 当前使用下文说明的随附生命周期脚本，本版本尚未把它注册到
`anolisa adapter enable` 的驱动集合。

## Adapter 处理规则

独立运行 `compress-response` 的默认值并不是大多数 Adapter 使用的默认值。共享 Adapter 按以下方式分类工具：

| 类别 | Adapter 默认行为 |
|------|------------------|
| 内容读取类，包括 Read/Glob/Grep/LSP/NotebookRead 别名 | 跳过响应压缩 |
| Shell/exec | 字符串 65,536 字符、数组保留 128 项、深度 8 |
| 其他结构化工具 | 字符串 1,048,576 字符、数组保留 65,536 项、深度 32 |

共享响应 Hook、OpenClaw 和 Hermes 会跳过短于 200 字符的输入。Codex 会跳过短于 500 字符的输入；只有输入至少为 4,000 字符时才附加压缩内容，否则只追加诊断或摘要。共享路径还会跳过带 YAML frontmatter、形似 Skill 的文本。TOON 编码仅对至少 500 字符的负载执行（当前实现的阈值，后续可能调整）；更小的负载保留压缩后的形式，因为 TOON 对小型 JSON 的节省可以忽略不计。该阈值适用于所有支持 TOON 的管线：共享响应 Hook、独立 TOON Hook、Codex、OpenClaw 和 Hermes。

Claude Code 需要 2.1.121 或更高版本才能使用 `updatedToolOutput`。版本更旧或无法确定时，响应压缩会关闭，以免重复注入原文。结构化工具输出会保留宿主 Schema，不会转换成文本 TOON；以字符串承载的 JSON 在 TOON 更小时可以使用 TOON。

### DeepSeek Harness 原生处理路径

DSH Bundle 要求 Node.js 22 或更高版本，并需要兼容的 DSH profile。应在同一条
enable 命令中列出全部目标 profile，随后使用其中一个名称启动 DSH。

```bash
anolisa adapter enable tokenless dsh \
  --profile web \
  --profile headless
dsh --profile web
```

`--profile` 是必填且可重复的参数。每次 enable 或 re-enable 都会把本次参数视为
完整目标集合。旧 receipt 中已有但新命令没有列出的 profile 会卸载 Bundle，因此
每次都要列出需要继续使用 Tokenless 的全部 profile。ANOLISA 会把选择的 profile
和解析后的 DSH home 写入 adapter receipt。后续 status、disable 和 re-enable 会
继续操作同一棵 profile 目录树。

Plugin 在 DSH 的 `tools/post-execute` waterfall 上运行。只有成功结果包含一个文本块，
且文本是 JSON object 或 array 时，才会尝试执行 `tokenless compress-response`。
CLI 返回更短的合法 JSON 后才会替换内容。多文本块、图片、普通文本、非法 JSON、
错误结果、Code Mode 子调用和默认内容读取类工具不参与压缩。CLI 缺失、失败或
超时也会保留原始内容。当前原生路径不执行 TOON 第二阶段，也没有启动子进程前的
最小尺寸门控。

在 `$DSH_HOME/profiles/<profile>/cordis.patch.yml` 中覆盖安装后的 row，然后重启
对应的 DSH profile。

```yaml
- id: anolisa-tokenless
  config:
    responseCompressionEnabled: true
    timeoutMs: 5000
    maxBuffer: 4194304
    noStash: false
```

后续 DSH patch layer 会替换该 row 的完整 `config` 值。Plugin 会为省略的 key 提供
默认值，因此只需写出准备修改的 key。

| 配置项 | 默认值 | 行为 |
|--------|--------|------|
| `responseCompressionEnabled` | `true` | 控制响应压缩。设为 `false` 后，环境错误归因仍保持启用。 |
| `tokenlessBin` | `$TOKENLESS_BIN`，随后使用 `tokenless` | 选择 Tokenless CLI 可执行文件。非空 Plugin 配置优先于环境变量。 |
| `skipTools` | 下文列出的内容读取类集合 | 跳过匹配工具的压缩。配置数组会替换默认集合，空数组表示不跳过任何工具。错误归因仍保持启用。 |
| `shellTools` | 下文列出的 Shell 和 process 集合 | 选择 Shell 阈值，也决定哪些工具的结构化 `value` 可以用于失败归因。配置数组会替换默认集合。 |
| `truncateStringsAt` | Shell 为 `65536`，其他工具为 `1048576` | 覆盖全部工具类别的字符串保留上限。只接受正整数。 |
| `truncateArraysAt` | Shell 为 `128`，其他工具为 `65536` | 覆盖全部工具类别的数组保留上限。只接受正整数。 |
| `maxDepth` | Shell 为 `8`，其他工具为 `32` | 覆盖全部工具类别的 JSON 最大深度。只接受正整数。 |
| `timeoutMs` | `3000` | 限制一次 Tokenless 子进程的运行时间，单位为毫秒。只接受正整数。 |
| `maxBuffer` | `2097152` | 限制捕获的子进程输出，单位为 byte。只接受正整数。 |
| `agentId` | `dsh` | 设置 Tokenless 统计记录中的 `--agent-id`。 |
| `noStash` | `false` | 设为 `true` 时传入 `--no-stash`。默认允许把删除的数组项写入 Stash。 |

默认 `skipTools` 集合包括 `Read`、`read`、`read_file`、`read_many_files`、`Glob`、
`glob`、`search_file`、`list_directory`、`list_dir`、`Grep`、`grep`、`grep_code`、
`grep_search`、`search_files`、`Lsp`、`lsp`、`NotebookRead`、`notebook_read` 和
`notebookread`。

默认 `shellTools` 集合包括 `Bash`、`bash`、`Shell`、`shell`、`exec`、`terminal`、
`run_shell_command`、`run_in_terminal`、`get_terminal_output`、`execute_command` 和
`process`。

DSH 使用 `isError` 标记的原始失败可以为任何工具追加依赖、权限、路径、网络或包
错误归因。结构化输出只会为 `shellTools` 分类。归因独立于压缩，关闭或跳过压缩、
压缩没有得到更短结果时仍会生效。后续 waterfall listener 替换 canonical `value`
后，Tokenless 会按替换值重新分类，不会沿用已经被替换结果的旧归因。

## 通过 anolisa 管理（推荐）

这些命令需要 ANOLISA 组件记录。如果 Tokenless 是通过 YUM 直接安装的，
继续操作前先记录该 RPM。

```bash
sudo yum install anolisa
sudo anolisa --install-mode system adopt tokenless
```

YUM 安装的 CLI 位于 `sudo` 可见的系统路径。`get.agentic-os.sh` 安装在用户
目录中的 CLI 可能会被 `sudo` 的 `secure_path` 隐藏。

后续 adapter 命令请用拥有目标 Agent 配置的用户执行。user scope 的 adapter
操作可以读取已采纳的 system 软件包，同时把框架改动留在当前用户的配置中。

### 1. 扫描 Agent 产品

```bash
anolisa adapter scan
```

如果目标框架未出现，先确认其 CLI 或应用已经安装，再重新扫描。

### 2. 启用一个 Adapter

```bash
anolisa adapter enable tokenless <framework>
```

例如：

```bash
anolisa adapter enable tokenless cosh
anolisa adapter enable tokenless openclaw
anolisa adapter enable tokenless hermes
anolisa adapter enable tokenless qoder
anolisa adapter enable tokenless claude-code
anolisa adapter enable tokenless codex
anolisa adapter enable tokenless qwencode
anolisa adapter enable tokenless dsh \
  --profile web \
  --profile headless
```

只需启用实际使用的 Agent 产品。多个产品应分别执行并验证各自的命令。DSH 的全部
目标 profile 应写在同一条 enable 命令中。

DeepSeek Harness 按 profile 管理，因此必须至少提供一个 `--profile`。每个名称应与
`dsh --profile <profile>` 使用的名称一致，不带 profile 的通用命令会被拒绝。
后续 enable 或 re-enable 必须再次列出需要保留的全部 profile。

OpenCode 应使用 [npm 安装后的手动接入](#npm-安装后的手动接入)中的随附安装脚本。

对于 OpenClaw，anolisa 会先尝试普通安装，默认不会加入 unsafe-install 覆盖参数。如果 OpenClaw 的安全扫描拒绝此 Plugin，应先阅读其报告；确认接受风险后，才显式重试：

```bash
anolisa adapter enable tokenless openclaw \
  --allow-unsafe-plugin-install
```

如果当前 OpenClaw 不支持底层覆盖参数，或已把它标记为无效的废弃选项，anolisa 会拒绝上述参数；此时应按照错误中的 `security.installPolicy` 指引处理。

组件软件包可以安装在 system scope，adapter receipt 仍由当前用户管理。只有
目标框架配置和 receipt 都明确归 root 所有时，才需要使用 `sudo`。

### 3. 检查状态

```bash
anolisa adapter status tokenless
anolisa doctor tokenless
```

完成后重启目标 Agent CLI 或 IDE。已经运行的会话通常不会动态载入刚安装的 Hook/Plugin。

### 4. 禁用

```bash
anolisa adapter disable tokenless <framework>
```

请用启用 adapter 的同一用户执行禁用操作。只有 root 管理的 receipt 需要在
两个操作中都使用 `sudo`。

禁用后重启目标 Agent。卸载 Tokenless 前必须先释放所有已启用的 Adapter。

## npm 安装后的手动接入

npm 的 postinstall 脚本会尝试把 Adapter 资源复制到：

```text
~/.local/share/anolisa/adapters/tokenless/
```

应确认该目录确实存在。Adapter 复制属于补充步骤，失败时只输出警告，不会让二进制安装失败；因此可能出现命令可用但这里没有资源副本的情况。目录缺失时应检查 npm postinstall 警告，并优先改用 anolisa 管理的安装。

npm 安装不会创建 anolisa 组件安装记录，因此不要假设 `anolisa adapter enable` 能管理这次安装。

### 使用 `tokenless init` 快速设置

安装 npm 包或从源码构建后，运行 `tokenless init` 检测已安装的 Agent 框架并引导适配器安装：

```bash
# 扫描并显示所有框架状态
tokenless init --list

# 安装指定适配器
tokenless init --framework claude-code

# 安装所有可安装的适配器（非交互模式）
tokenless init --all

# 交互式选择（stdin 为终端时）
tokenless init
```

`tokenless init` 读取适配器清单，运行各框架的 `detect.sh`，报告适配器状态（就绪 / 可安装 / 缺少前置依赖），然后为选定框架执行 `install.sh`。

### 手动安装脚本

OpenClaw、Hermes、Qoder、Claude Code、Codex、OpenCode 和 Qwen Code 可以运行各自的安装脚本：

```bash
bash ~/.local/share/anolisa/adapters/tokenless/<framework>/scripts/install.sh
```

例如：

```bash
bash ~/.local/share/anolisa/adapters/tokenless/claude-code/scripts/install.sh
bash ~/.local/share/anolisa/adapters/tokenless/opencode/scripts/install.sh
```

卸载相同 Adapter：

```bash
bash ~/.local/share/anolisa/adapters/tokenless/<framework>/scripts/uninstall.sh
```

脚本会调用框架自身的 Plugin/Extension 机制；按照脚本输出完成重启。安装脚本缺失、失败或框架版本不兼容时，优先改用 anolisa 管理的安装方式。

OpenClaw 安装脚本会带 `--dangerously-force-unsafe-install` 调用 `plugins install`，因为 Plugin 通过 Node.js 子进程 API 启动 `tokenless` 和 `rtk` 二进制。运行前应审查已安装的 Adapter 源码和 OpenClaw 安全策略。如果策略不允许该覆盖参数，就不要安装此 Plugin。

### npm + cosh

cosh 使用 Extension 目录，不提供单独的 `scripts/install.sh`。将 npm 安装的共享资源复制到 cosh 的用户 Extension 目录：

```bash
mkdir -p ~/.copilot-shell/extensions/tokenless
cp -R ~/.local/share/anolisa/adapters/tokenless/common/hooks \
  ~/.local/share/anolisa/adapters/tokenless/common/commands \
  ~/.local/share/anolisa/adapters/tokenless/common/cosh-extension.json \
  ~/.copilot-shell/extensions/tokenless/
```

完成后重启 cosh。移除前先退出 cosh，并确认目标目录确实是本次 npm 安装创建的 Tokenless Extension。

## Agent Adapter 生效提示

### cosh

Extension 在启动时发现。启用后重启 cosh，并运行一个 Shell 工具任务，再使用 `tokenless stats list` 检查记录。

### OpenClaw

安装脚本会使用上文说明的 OpenClaw unsafe-install 覆盖参数。确认风险并安装后，重启 Gateway。Plugin 代码默认启用响应压缩和 RTK 重写，默认关闭 TOON。由于底层检查已硬关闭，Plugin 的 Tool Ready 选项当前不会生效。

### Hermes

Plugin 在 Hermes 新会话中生效。重启 Hermes 后执行一个 Shell 工具任务验证。

### Qoder

Qoder IDE 和 qodercli 可能缓存 Plugin 配置。启用或升级后应完全重启 IDE。若出现旧 Hook 路径错误，参阅[Qoder Plugin 缓存问题](troubleshooting.md#qoder-plugin-缓存问题)。

### Claude Code

Marketplace Plugin 在 Claude Code 重启后生效，也可以按照安装脚本提示刷新 Plugin。

### Codex

Plugin 在新的 Codex 会话中加载。关闭旧会话并重新启动后验证统计。它的 PostToolUse Hook 是追加型的：统计只能作为压缩候选遥测，不能证明原始 Codex 工具结果已离开 Prompt。

### DeepSeek Harness

原生 Bundle 会在选定的 DSH profile 启动时加载。启用 Bundle 或修改 profile patch
后，重启 `dsh --profile <profile>`，运行一个返回可压缩 JSON 的工具，再检查
`tokenless stats list`。禁用命令是 `anolisa adapter disable tokenless dsh`。
receipt 已经记录 profile 名称，因此 disable 不再接受 `--profile`。

### OpenCode

OpenCode 启动时会自动加载配置目录下的 Plugin。使用上述 Tokenless 生命周期脚本
完成安装或卸载后，请重启 OpenCode。重启后执行一次工具调用，再运行
`tokenless stats list`，确认已生成统计记录。

脚本会优先使用 `TOKENLESS_OPENCODE_CONFIG_DIR`，其次使用
`OPENCODE_CONFIG_DIR`。如果两者均未设置，则使用
`${XDG_CONFIG_HOME}/opencode`；如果 `XDG_CONFIG_HOME` 也未设置，则回退到
`~/.config/opencode`。

安装过程中，脚本只会创建由 Tokenless 管理的 `plugins/tokenless.js` 符号链接。
如果目标路径已经存在但不由 Tokenless 管理，安装会停止，原有内容不会被覆盖。

### Qwen Code

Extension 在新的 Qwen Code 会话中加载。重启后执行一次工具调用验证。

## AgentScope 框架集成

Python 包支持 AgentScope 1.0.11 至 1.0.x 和 AgentScope 2.0.x。应根据已安装版本选择
挂载入口：

| AgentScope 版本 | 支持的入口 |
|---|---|
| 1.0.11 至 1.0.x | 使用 Tokenless Toolkit 和 `install(..., session_id=...)` |
| 2.0.0 | 通过 `integration.tools` 和 `integration.middlewares` 直接构造 Agent |
| 2.0.1 至 2.0.x | 直接构造 Agent，或通过 `integration.app_options()` 接入 App |

原生 `anolisa-tokenless` Runtime Wheel 和 AgentScope 集成 Wheel 当前都尚未发布到
Python 包索引。请从源码 checkout 构建并同时安装两个相同版本的 Wheel：

```bash
make python-wheel agentscope-wheel
python -m pip install \
  target/wheels/anolisa_tokenless-*.whl \
  target/wheels/anolisa_tokenless_agentscope-*.whl
```

原生 Wheel 还通过 typed Python 对象开放与 CLI 相同的只读 Stats 查询能力：

```python
from anolisa_tokenless import TokenlessStats

stats = TokenlessStats("/absolute/path/to/tenant-tokenless-data")

status = stats.status
summary = stats.summary()
recent = stats.list(limit=20)
record = stats.show(recent[0].id)
session_diff = stats.diff(session_id="conversation-id")
comparison = stats.compare("baseline-session", "tokenless-session")
```

`TokenlessSdk.stats` 会延迟返回绑定该 SDK 数据目录的客户端。Token 数量是估算值，并且
只有产生正向节省的操作才会记录。`show()` 和 Record/Tool-use 的 `diff()` 结果可能包含
`stats.db` 中保存的敏感工具输入与输出；Summary、List 和 Compare 不返回保存的内容。
该 API 不能清空数据或修改记录开关。这里的只读是指这些公开操作；打开客户端时遵循
CLI 初始化流程，可能创建或迁移 `stats.db`，所以数据目录必须可写。Summary 或 Compare
未指定 Limit 时，最多读取最近 10,000 条记录；Session 和 Tool-use Diff 同样最多读取
最近 10,000 条匹配记录。要获得有意义的对比，应先传入 dry-run Baseline Session，再
传入启用 Tokenless 的 Session。

两个大版本都使用 `TokenlessAgentScope` 和 `TokenlessConfig`，只有最后的挂载方式不同。
AgentScope 1.x 使用 Tokenless Toolkit；普通工具与 MCP 注册入口也会覆盖构造后新增的工具：

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

AgentScope 2.x 应在构造 Toolkit 和 Agent 时传入恢复 Tool 和中间件。该方式从 2.0.0
即可使用，不依赖后续补丁版本才引入的 Toolkit 动态修改 API：

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

AgentScope App 从 2.0.1 开始支持。`app_options()` 会在配置的绝对基础目录下，为每个
user/agent/session 派生独立的 Tokenless 数据目录：

```python
from agentscope.app import create_app
from tokenless_agentscope import TokenlessAgentScope, TokenlessConfig

integration = TokenlessAgentScope(
    TokenlessConfig(data_dir="/srv/tokenless-tenants"),
)
app = create_app(..., **integration.app_options())
```

如果应用已经定义 `tokenless_retrieve`，应在 `TokenlessConfig` 中设置唯一的
`retrieve_tool_name`；App 组装阶段不会把其他工具暴露给该 factory，无法预先检查重名。
AgentScope 2.0.0 尚未提供 App 级 Agent middleware 和 Tool 注入，因此只支持直接构造
Agent。原有 `TokenlessMiddleware` 2.x API 继续保留兼容；新代码应使用
`TokenlessAgentScope`，避免依赖特定补丁版本的 Toolkit 动态修改或 Tool 自动收集行为。

请根据应用可接受的 inline 截断程度选择模式：

| 模式 | Read/Glob/Grep | 其他工具 |
|------|----------------|----------|
| `conservative` | 压缩 | 字符串 1 MiB、数组 65,536 项、深度 32 |
| `balanced`（默认） | 跳过 | Shell：65,536 / 128 / 深度 8；其他采用 conservative 限制 |
| `aggressive` | 跳过 | CLI 默认值：4,096 / 32 / 深度 8 |

集成会原样转发中间流式 chunk 并保留框架对象，只转换复制后的调用参数和最终模型可见
文本。Tokenless 优化失败或 UTF-8 结果没有严格变小时保留原文，`DataBlock` 永不修改。

集成还提供默认名为 `tokenless_retrieve` 的恢复 Tool。只有 marker 对当前模型可见时才
会向模型发布该 Tool，并且只接受该 Session 精确保留的 marker 集合中的 24 位十六进制
hash；该 Tool 永远不参与压缩。这一窄权限仍依赖
存储隔离：每个用户或租户必须显式传入独立的绝对 `data_dir`。省略 `data_dir` 时，
`TOKENLESS_DATA_DIR` 只作为进程级回退，不得由多个租户共用；也不要依赖跨节点恢复。
stash 当前使用固定的一小时 TTL，Agent 应在这一边界前恢复所需内容。

两个 Adapter 都启用 Schema 压缩、RTK 命令改写、响应压缩、TOON、恢复、环境错误提示
和逐调用归属。平台 Wheel 内置 RTK 并直接链接 TOON，不会搜索系统 helper。Tool Ready
仍保持硬关闭。

## 验证是否真正接入

对于 Agent Adapter，不要只以“安装命令退出码为 0”作为成功标准。至少完成：

```bash
tokenless --version
anolisa adapter status tokenless
tokenless stats list --limit 5
```

然后在目标 Agent 中执行一次有明显输出的工具任务。如果 `stats list` 仍为空，请按照[启用后没有产生统计记录](troubleshooting.md#启用后没有产生统计记录)排查。

对于 AgentScope 框架包，在源码 checkout 中运行下面的命令，验证两个 Wheel 和声明
支持的 AgentScope 版本范围：

```bash
make test-agentscope-integration
```

随后在应用中执行一次成功且可压缩的工具响应，确认中间件返回更小的结果，并确认
`tokenless_retrieve` 可以从同一个 `data_dir` 恢复 marker 对应的内容。

## 相关文档

- [快速开始](QUICKSTART.md)
- [效果度量](measuring-savings.md)
- [配置与数据隐私](configuration-and-privacy.md)
- [故障排查](troubleshooting.md)
