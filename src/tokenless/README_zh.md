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

## 快速开始

```bash
# 完整安装：构建 + 安装二进制 + 部署所有适配器
make setup
```

安装完成后 `tokenless` 命令位于 `~/.local/bin`，RTK/TOON 辅助二进制同目录。

## 架构

- `crates/tokenless-schema/` — 核心库：SchemaCompressor + ResponseCompressor
- `crates/tokenless-ccr/` — 可逆压缩缓存（Compress-Cache-Retrieve）
- `crates/tokenless-cli/` — CLI 二进制
- `adapters/tokenless/` — 适配器包（OpenClaw / Hermes / Qoder / Claude Code / Codex）
- `third_party/rtk/` — RTK 命令重写引擎（vendored）

## 许可证

Apache License 2.0 — 详见 [LICENSE](../../LICENSE)。
