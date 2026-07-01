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
- 已安装 cosh 或 OpenClaw 之一（作为宿主 Agent 框架）

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

Tokenless 通过 hook 或 plugin 与 Agent 框架集成。

### cosh（Copilot Shell）

安装 cosh hook：

```bash
/usr/share/tokenless/scripts/install.sh --cosh
```

安装后，Tokenless 将自动压缩 cosh 会话中的工具 Schema 和 CLI 输出。

### OpenClaw

安装 OpenClaw 插件：

```bash
/usr/share/tokenless/scripts/install.sh --openclaw
```

插件注册为 OpenClaw 工具管道中的中间件层。

---

## 使用

### 查看压缩统计

```bash
tokenless stats list
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

无需额外配置——指标自动导出。

---

## 配置

配置文件：`~/.config/tokenless/config.toml`

```toml
[compression]
# 压缩等级："aggressive"、"balanced"、"conservative"
level = "balanced"

[stats]
# 启用统计收集
enabled = true

[integration]
# 自动导出到 AgentSight
agentsight = true
```

---

## 常见问题

**Q：Tokenless 会修改实际的工具行为吗？**
A：不会。Tokenless 仅压缩发送给模型的表示形式，工具执行逻辑不受影响。

**Q：支持哪些框架？**
A：目前支持 cosh 和 OpenClaw，Hermes 支持计划中。

**Q：能否对特定工具禁用压缩？**
A：可以。在配置文件的 `[compression.exclude]` 列表中添加工具名称即可。
