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
| Tool Ready | 减少重试浪费 | 预检环境、自动修复依赖、故障归因 |

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
- **响应本就短小**：压缩后 `after >= before`，CLI 输出原文且不记录统计（属正常）。
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

- **OpenClaw 插件** — 命令重写 + 响应压缩 + Schema 压缩
- **copilot-shell 钩子** — Tool Ready + 命令重写 + 响应压缩 + TOON
- **Hermes Agent 插件** — Tool Ready + 命令重写 + 响应压缩 + TOON
- **Qoder CLI 插件** — Tool Ready + 命令重写 + 响应压缩
- **Claude Code 插件** — Tool Ready + 命令重写 + 响应压缩 + TOON
- **Codex 插件** — Tool Ready + 命令重写 + 响应压缩 + TOON
- **OpenCode 插件** — Tool Ready + 命令重写 + Schema/响应压缩 + TOON

## 快速开始

首选 ANOLISA CLI 安装已发布的组件。

安装脚本会把 `anolisa` 放到 `~/.local/bin`。user mode 安装的 `tokenless`、
`rtk` 和 `toon` 也在这个目录。如果当前 Shell 还找不到命令，先把该目录加入
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
直接改动 Agent 框架的用户配置。请用拥有该配置的用户执行以下命令，并且只启用
准备使用的 adapter。

```bash
anolisa adapter scan
anolisa adapter enable tokenless openclaw
anolisa adapter status tokenless
```

从源码构建适合开发者。

```bash
git clone <repo-url>
cd Token-Less

# 完整安装，构建并安装二进制，随后部署所有 adapter
make setup
```

源码安装会把 `tokenless` 放在 `~/.local/bin`，`rtk` 和 `toon` 辅助
二进制也位于同一个目录，并部署开发所需的全部 adapter。

### OpenCode 安装

OpenCode 适配器通过 `tool.execute.before/after` 原生插件事件执行 Tool Ready、
RTK 命令重写和响应/TOON 压缩，并通过 `tool.definition` 压缩工具 Schema。
压缩后的响应会替换原始模型可见输出，避免重复占用上下文。

```bash
make opencode-install
```

安装器会在 OpenCode 全局 `plugins/` 目录中创建 `tokenless.js` 符号链接，
不会覆盖同名的非托管文件。配置目录支持 `OPENCODE_CONFIG_DIR`、
`XDG_CONFIG_HOME` 和显式的 `TOKENLESS_OPENCODE_CONFIG_DIR` 覆盖。
安装后重启 OpenCode 即可加载插件。

## Raw 打包

Raw 打包接收同一目录中已经构建好的 `tokenless`、`rtk`、`toon`，并按照
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

该目录必须是位于真实用户 home 下的绝对路径。若只需自定义一个数据库，
现有的 `TOKENLESS_STATS_DB`、`TOKENLESS_STASH_DB` 和 `--stash-db` 覆盖项
优先级更高。配置文件仍位于 `~/.tokenless/config.json`。

## 架构

- `crates/tokenless-schema/` — 核心库：SchemaCompressor + ResponseCompressor
- `crates/tokenless-ccr/` — 可逆压缩缓存（Compress-Cache-Retrieve）
- `crates/tokenless-cli/` — CLI 二进制
- `adapters/tokenless/` — 适配器包（OpenClaw / Hermes / Qoder / Claude Code / Codex / OpenCode）
- `third_party/rtk/` — RTK 命令重写引擎（vendored）
- `packaging/raw/` — Tokenless 自维护的 ANOLISA Raw 打包与目标校验

## 许可证

Apache License 2.0 — 详见 [LICENSE](../../LICENSE)。
