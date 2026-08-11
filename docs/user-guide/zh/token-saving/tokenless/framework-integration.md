# Tokenless 框架集成

[English](../../../en/token-saving/tokenless/framework-integration.md)

Tokenless 通过 Adapter 把压缩、命令重写和环境检查接入 Agent。安装 Tokenless 只提供二进制和 Adapter 资源；启用 Adapter 后，目标 Agent 才会自动调用这些能力。

## 支持矩阵

| 框架 | 值 | Tool Ready | 命令重写行为 | 响应交付方式 | TOON | Schema |
|------|----|------------|--------------|--------------|------|--------|
| cosh | `cosh` | ✅ | 替换受支持的 Shell 输入 | Cosh-NG 替换响应；旧版 Copilot Shell 追加上下文 | 在响应压缩后尝试 | ✅ |
| OpenClaw | `openclaw` | ✅ | 替换 `exec` 命令输入 | 替换持久化工具结果消息 | 默认关闭，需主动启用 | — |
| Hermes | `hermes` | ✅ | 阻止第一次调用并要求 Agent 重试 | 替换结果字符串 | 在响应压缩后尝试 | — |
| Qoder | `qoder` | ✅ | 输出改写后的 Shell 输入 | 输出 `additionalContext` | 在响应压缩后尝试 | — |
| Claude Code | `claude-code` | ✅ | 替换 Bash 输入 | 2.1.121 及以上替换输出；否则透传 | 仅在替换结果可保持文本时使用 | — |
| Codex | `codex` | ✅ | 替换受支持的 Shell 输入 | 保留原文，追加分析或压缩备选内容 | 用于生成该备选内容 | — |
| OpenCode | `opencode` | ✅ | 替换 Bash 输入 | 替换工具输出 | 在响应压缩后尝试 | ✅ |
| Qwen Code | `qwencode` | ✅ | 输出改写后的 Shell 输入 | 输出 `additionalContext` | 在响应压缩后尝试 | ✅ |

“—”表示当前 Adapter 没有注册此能力；对应的 Tokenless CLI 命令仍可能可用。

`additionalContext` 是追加型 Hook 字段。在这些路径上，Tokenless 源码本身不会删除原始结果，最终处理方式还取决于宿主实现。统计记录只能证明压缩候选内容变小了，不能证明宿主已经从模型请求中移除原文。

OpenCode 当前使用下文说明的随附生命周期脚本；本版本尚未把它注册到 `anolisa adapter enable` 的驱动集合。

## Adapter 处理规则

独立运行 `compress-response` 的默认值并不是大多数 Adapter 使用的默认值。共享 Adapter 按以下方式分类工具：

| 类别 | Adapter 默认行为 |
|------|------------------|
| 内容读取类，包括 Read/Glob/Grep/LSP/NotebookRead 别名 | 跳过响应压缩 |
| Shell/exec | 字符串 65,536 字符、数组保留 128 项、深度 8 |
| 其他结构化工具 | 字符串 1,048,576 字符、数组保留 65,536 项、深度 32 |

共享响应 Hook、OpenClaw 和 Hermes 会跳过短于 200 字符的输入。Codex 会跳过短于 500 字符的输入；只有输入至少为 4,000 字符时才附加压缩内容，否则只追加诊断或摘要。共享路径还会跳过带 YAML frontmatter、形似 Skill 的文本。

Claude Code 需要 2.1.121 或更高版本才能使用 `updatedToolOutput`。版本更旧或无法确定时，响应压缩会关闭，以免重复注入原文。结构化工具输出会保留宿主 Schema，不会转换成文本 TOON；以字符串承载的 JSON 在 TOON 更小时可以使用 TOON。

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

### 1. 扫描框架

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
```

只需启用实际使用的框架。为多个框架启用时，应逐个执行并分别验证。

OpenCode 不适用本节，应使用 [npm 安装后的手动接入](#npm-安装后的手动接入)中的随附安装脚本。

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

## 各框架的生效提示

### cosh

Extension 在启动时发现。启用后重启 cosh，并运行一个 Shell 工具任务，再使用 `tokenless stats list` 检查记录。

### OpenClaw

安装脚本会使用上文说明的 OpenClaw unsafe-install 覆盖参数。确认风险并安装后，重启 Gateway。Plugin 代码默认启用响应压缩、Tool Ready 和 RTK 重写，默认关闭 TOON。

### Hermes

Plugin 在 Hermes 新会话中生效。重启 Hermes 后执行一个 Shell 工具任务验证。

### Qoder

Qoder IDE 和 qodercli 可能缓存 Plugin 配置。启用或升级后应完全重启 IDE。若出现旧 Hook 路径错误，参阅[Qoder Plugin 缓存问题](troubleshooting.md#qoder-plugin-缓存问题)。

### Claude Code

Marketplace Plugin 在 Claude Code 重启后生效，也可以按照安装脚本提示刷新 Plugin。

### Codex

Plugin 在新的 Codex 会话中加载。关闭旧会话并重新启动后验证统计。它的 PostToolUse Hook 是追加型的：统计只能作为压缩候选遥测，不能证明原始 Codex 工具结果已离开 Prompt。

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

## 验证是否真正接入

不要只以“安装命令退出码为 0”作为成功标准。至少完成：

```bash
tokenless --version
anolisa adapter status tokenless
tokenless stats list --limit 5
```

然后在目标 Agent 中执行一次有明显输出的工具任务。如果 `stats list` 仍为空，请按照[启用后没有产生统计记录](troubleshooting.md#启用后没有产生统计记录)排查。

## 相关文档

- [快速开始](QUICKSTART.md)
- [效果度量](measuring-savings.md)
- [配置与数据隐私](configuration-and-privacy.md)
- [故障排查](troubleshooting.md)
