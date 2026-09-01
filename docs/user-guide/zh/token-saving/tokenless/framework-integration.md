# Tokenless Agent 集成

[English](../../../en/token-saving/tokenless/framework-integration.md)

Tokenless 通过 Plugin、Hook 和 Extension 接入具体 Agent 产品。本文只说明产品 Adapter。
Python SDK 及其 AgentScope 专用子文档放在 [Python SDK 指南](sdk.md) 下。

## Agent Adapter 支持矩阵

| Agent 产品 | 值 | Tool Ready | 命令重写行为 | 响应交付方式 | TOON | Schema |
|------|----|------------|--------------|--------------|------|--------|
| cosh | `cosh` | 已硬关闭 | 替换受支持的 Shell 输入 | Cosh-NG 替换无损 JSON 结果；旧版 Copilot Shell 透传 | 对可替换文本由 Pipeline 选择 | Common Hook 仅接受无损结果 |
| OpenClaw | `openclaw` | 已硬关闭 | 替换 `exec` 命令输入 | 替换持久化工具结果消息 | 默认关闭，需主动启用 | — |
| Hermes | `hermes` | 已硬关闭 | 阻止第一次调用并建议使用 Core 返回的改写命令 | 替换已接受的无损结果或追加错误指引 | 对可替换文本由 Core 选择 | — |
| Qoder | `qoder` | 已硬关闭 | 输出改写后的 Shell 输入 | 通过 `updatedToolOutput` 替换输出 | 对可替换文本由 Pipeline 选择 | — |
| Claude Code | `claude-code` | 已硬关闭 | 替换 Bash 输入 | 2.1.121 及以上替换输出；否则透传 | 对可替换文本由 Pipeline 选择 | — |
| Codex | `codex` | 已硬关闭 | 替换受支持的 Shell 输入 | 保留原文；仅对识别出的环境失败追加上下文 | — | — |
| DeepSeek Harness | `dsh` | 未注册 | 未注册 | 只在结果更小时替换已接受的单文本块 JSON 结果 | 未注册 | 未注册 |
| OpenCode | `opencode` | 已硬关闭 | 替换 Bash 输入 | 替换工具输出 | 对可替换文本由 Pipeline 选择 | ✅ |
| Qwen Code | `qwencode` | 已硬关闭 | 输出改写后的 Shell 输入 | 宿主没有替换字段，因此透传 | — | — |

“—”表示该能力不可用：当前 Adapter 没有注册，或当前宿主版本不会运行；对应的 Tokenless CLI 命令仍可能可用。

Schema 压缩到达模型路径的方式因宿主而异：cosh 与 Cosh-NG 触发 `BeforeModel` Hook；OpenCode 通过其 `tool.definition` 插件 Hook 逐个压缩工具定义（MCP 工具不经过该 Hook）；Qwen Code 的清单声明了 `BeforeModel` Hook，但当前 Qwen Code 版本在注册时会跳过这一未知事件名，Schema Hook 实际不会运行，因此矩阵标记为不可用。该条目保留注册，未来 Qwen Code 版本实现该事件后会自动生效。

这些 Adapter 仍会注册 Tool Ready，但会在检查、修复或阻断之前无条件硬退出，任何运行时设置都无法重新启用。工具执行后的失败归因不受影响。

`additionalContext` 是追加型 Hook 字段。共享 Hook 不会把压缩副本放入其中，否则原文
仍然可见，总 Context 反而增加；该字段只用于追加环境错误指引。统计记录只能证明压缩候选
内容变小了，不能单独证明宿主已经从模型请求中移除原文。

## Adapter 处理规则

共享 Cosh-NG、Qoder、Claude Code 和 OpenCode PostTool Hook 会向 `tokenless compress`
发送一个 Protocol v2 `post_tool` 请求。它声明输出替换能力，但不声明受信 Agent-facing
Retrieve 能力。因此 Core 只应用无损 JSON 候选，并对所有非 `applied` Disposition 返回原文。
当前路由如下：

| 内容 | 当前共享 Hook 行为 |
|------|--------------------|
| JSON | 无损结构清理；文本替换槽还会考虑 TOON |
| 需要字符串、数组或深度截断的 JSON | Common Hook 无法发布经过授权的 Retrieve Tool，因此以 `recoverability_unavailable` 拒绝候选 |
| 构建/测试/包管理日志、长纯文本、Diff、Stack Trace、HTML、搜索结果、表格、源码、Unknown | 对应领域 Compressor 接入前原样透传 |

内容检测、PostTool 200 字符门禁、基于工具来源的阈值、诊断、TOON 选择和最终接受均属于
Core 策略。Hook 只把宿主对象映射为 v2 字段；它可以跳过明显不是 JSON 的 Skill 文件，避免
无意义地启动子进程。

Common BeforeModel Hook 同样使用 Protocol v2，并声明没有受信 Retrieve 能力。只有 Core
证明结果无损时才返回变换后的 Tools。当前 `SchemaCompressor` 的每一种变换都会移除或改写
Schema 信息，因此这条 Common 路径会原样返回 Tools、不产生 Schema 压缩 Stats 记录，也不会
发出不可恢复 Marker。OpenCode 独立的逐工具定义路径和直接 `compress-schema` 命令不受影响。

OpenClaw 与 Hermes 已把 PostTool 决策委托给 Core。DeepSeek Harness 仍使用专用响应路径；
content-aware build/log 路径尚未接入。独立 `compress-response` 命令也继续作为显式 JSON
清理入口。

对于 JSON 响应清理，共享与旧版 Adapter 按以下方式分类工具：

| 类别 | Adapter 默认行为 |
|------|------------------|
| 内容读取类，包括 Read/Glob/Grep/LSP/NotebookRead 别名 | 跳过响应压缩 |
| Shell/exec | 字符串 65,536 字符、数组保留 128 项、深度 8 |
| 其他结构化工具 | 字符串 1,048,576 字符、数组保留 65,536 项、深度 32 |

Common Hook、OpenClaw 与 Hermes 的 PostTool 大小门禁、基于工具来源的阈值和 TOON 选择均归
Core。只有宿主槽支持文本且 Core 找到更小的合法表示时才会使用 TOON。独立
`compress-toon` CLI 和 SDK TOON 路径继续使用文档规定的默认门槛，CLI 可通过
`--min-toon-chars` 为单次调用降低阈值。Codex 和 Qwen Code 当前的 PostToolUse 契约不能
替换原始模型可见输出，因此不运行响应压缩或 TOON。

Common Hook 与 OpenClaw 会把 RTK 所有权传给匹配的 PostTool 调用。Hermes 为兼容旧宿主版本
采用阻止后建议重试；最终结果 Hook 会从 Hermes 实际执行的命令中识别带 Attribution 的 RTK
Wrapper。因此三者都会让 RTK 输出绕过第二次压缩。

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
anolisa adapter enable tokenless opencode
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

npm 安装不会创建 anolisa 组件安装记录，因此不要假设 `anolisa adapter enable` 能管理这次安装。OpenClaw、Hermes、Qoder、Claude Code、Codex、OpenCode 和 Qwen Code 可以运行各自的安装脚本：

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

Plugin 在 Hermes 新会话中生效。重启 Hermes 后先执行 Shell 工具任务验证阻止后重试改写，
再执行返回 JSON 的工具验证结果替换。Hermes 无法发布受信 Retrieve Tool，因此需要恢复能力
的压缩会原样透传。

### Qoder

Qoder IDE 和 qodercli 可能缓存 Plugin 配置。启用或升级后应完全重启 IDE。若出现旧 Hook 路径错误，参阅[Qoder Plugin 缓存问题](troubleshooting.md#qoder-plugin-缓存问题)。

### Claude Code

Marketplace Plugin 在 Claude Code 重启后生效，也可以按照安装脚本提示刷新 Plugin。

### Codex

Plugin 在新的 Codex 会话中加载。关闭旧会话并重新启动后验证行为。Codex 的 PostToolUse Hook 不能替换或抑制原始输出，因此 Plugin 不追加压缩内容，也不记录响应压缩候选，只对识别出的环境失败追加上下文。真正的首轮节省来自 RTK 在执行前重写受支持的 Shell 命令。

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

AgentScope 是 Python SDK 的第二层，不是产品 Adapter。完整的构建、版本、挂载、配置与验证说明
现已放在 [AgentScope SDK 集成](sdk/agentscope.md) 子文档。本标题继续保留，作为已有链接的兼容入口。

## 验证 Agent Adapter

对于 Agent Adapter，不要只以“安装命令退出码为 0”作为成功标准。至少完成：

```bash
tokenless --version
anolisa adapter status tokenless
tokenless stats list --limit 5
```

然后在目标 Agent 中执行一次有明显输出的工具任务。如果 `stats list` 仍为空，请按照[启用后没有产生统计记录](troubleshooting.md#启用后没有产生统计记录)排查。

## 相关文档

- [快速开始](QUICKSTART.md)
- [Python SDK](sdk.md)
- [AgentScope SDK 集成](sdk/agentscope.md)
- [效果度量](measuring-savings.md)
- [配置与数据隐私](configuration-and-privacy.md)
- [故障排查](troubleshooting.md)
