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
