# Tokenless

Tokenless 是 ANOLISA 的 Token 优化组件。它自动压缩工具定义和模型响应内容，不修改任何业务逻辑，显著降低每轮对话的 Token 消耗。

---

## 概述

AI Agent 交互中通常包含大量工具 Schema 定义和冗长的 CLI 输出。Tokenless 在框架层拦截这些内容并进行无损/近无损压缩，透明地实现 30–70% 的 Token 节省。

**核心能力：**

- **上下文压缩** — 工具 Schema 精简、CLI 响应过滤、紧凑编码
- **统计追踪** — 按会话和累计的 Token 节省指标
- **透明集成** — 通过 hook/plugin 接入现有 Agent 框架，零代码修改

---

## 前置条件

- Linux（x86_64 或 aarch64）
- 已安装 cosh、OpenClaw、Hermes、claude-code、codex 或 qwencode 之一（作为宿主 Agent 框架）

---

## 安装

### 方式一：anolisa CLI（推荐）

```bash
anolisa install tokenless
```

### 方式二：YUM（Alinux，需配置 ANOLISA YUM 源）

```bash
sudo yum install tokenless
```

### 方式三：源码编译（开发者）

```bash
cd src/tokenless && cargo build --release
```

---

## 集成

Tokenless 通过 adapter 脚本或 extension 与 Agent 框架集成。

### OpenClaw

安装 OpenClaw adapter：

```bash
/usr/share/anolisa/adapters/tokenless/openclaw/scripts/install.sh
```

adapter 注册为 OpenClaw 工具管道中的中间件层。

### Hermes

安装 Hermes adapter：

```bash
/usr/share/anolisa/adapters/tokenless/hermes/scripts/install.sh
```

### cosh（Copilot Shell）

对于 cosh，Tokenless 以 extension 方式安装：

```bash
# 通过 Makefile 目标
make install-cosh-extension
```

将安装到 `~/.copilot-shell/extensions/tokenless/`。

### 其他 Adapter

Tokenless 还支持 claude-code、codex 和 qwencode adapter。运行 `anolisa install tokenless` 查看可用的 adapter 选项。

---

## CLI 命令

| 命令 | 说明 |
|------|------|
| `tokenless compress-schema` | 压缩工具 Schema 定义 |
| `tokenless compress-response` | 压缩 CLI/工具响应输出 |
| `tokenless compress-toon` | 压缩为 TOON 格式 |
| `tokenless decompress-toon` | 从 TOON 格式解压 |
| `tokenless env-check` | 检查环境和集成状态 |
| `tokenless stats` | 查看压缩统计 |

### 查看压缩统计

```bash
tokenless stats
```

示例输出：

```
Session       Tokens Saved   Ratio    Timestamp
────────────  ────────────   ─────    ──────────────────
sess-a3f1     12,480         62.3%    2025-06-30 14:22
sess-b7c2      8,912         48.7%    2025-06-30 15:01
────────────────────────────────────────────────────────
Total         21,392         56.1%
```

---

## AgentSight 集成

当 Tokenless 和 AgentSight 同时安装时，压缩指标将自动上报到 AgentSight。可在 AgentSight Web Dashboard 的 **Token Accounting** 面板中查看 Token 节省数据。

`sls_enabled` 为 true 时指标自动导出，无需额外配置。

---

## 配置

配置文件：`~/.tokenless/config.json`

该文件为可选项。不存在时所有功能默认启用。

```json
{
  "stats_enabled": true,
  "sls_enabled": true,
  "compression_enabled": true
}
```

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `stats_enabled` | boolean | `true` | 启用本地统计收集（存储于 `~/.tokenless/stats.db`） |
| `sls_enabled` | boolean | `true` | 启用指标导出到 AgentSight/SLS |
| `compression_enabled` | boolean | `true` | 启用压缩（全局开关，非按工具粒度） |

### 环境变量覆盖

每个配置字段均可通过环境变量覆盖：

- `TOKENLESS_STATS_ENABLED` — 覆盖 `stats_enabled`
- `TOKENLESS_SLS_ENABLED` — 覆盖 `sls_enabled`
- `TOKENLESS_COMPRESSION_ENABLED` — 覆盖 `compression_enabled`

### 统计数据库

本地统计数据存储于 `~/.tokenless/stats.db`。

---

## 常见问题

**Q：Tokenless 会修改实际的工具行为吗？**
A：不会。Tokenless 仅压缩发送给模型的表示形式，工具执行逻辑不受影响。

**Q：支持哪些框架？**
A：cosh、OpenClaw、Hermes、claude-code、codex 和 qwencode。

**Q：能否禁用压缩？**
A：可以。将 `~/.tokenless/config.json` 中的 `compression_enabled` 设为 `false`，或设置环境变量 `TOKENLESS_COMPRESSION_ENABLED=false`。压缩是全局开关，不支持按工具排除。
