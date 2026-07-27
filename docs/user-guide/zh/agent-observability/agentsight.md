# AgentSight

AgentSight 是基于 eBPF 的 AI Agent 可观测性工具，在零侵入业务逻辑的前提下，实现对 Agent 运行全链路的细粒度数据采集与关联分析。

## 概述

AgentSight 为运行在 Linux 上的 AI Agent 提供全栈可观测能力：

| 能力 | 说明 |
|------|------|
| Token 消耗分析 | 按 Agent、任务、模型等多维度 Token 计账 |
| 行为审计 | LLM 调用与进程执行行为的全链路记录 |
| Dashboard 可视化 | Web UI 实时展示 Token 趋势、Agent 状态与会话追踪 |
| Agent 自动发现 | 自动检测系统中运行的 AI Agent 进程 |
| 中断检测 | 检测 LLM 错误、SSE 截断、上下文溢出、进程崩溃等异常 |
| 外部日志导出 | 支持将结构化事件导出到外部日志服务 |

## 前置条件

| 条件 | 最低要求 |
|------|----------|
| OS | Linux |
| 内核 | >= 5.8（需要 BTF 支持） |
| 权限 | root 或 CAP_BPF（eBPF 探针） |
| 架构 | x86_64 / aarch64 |

## 安装

```bash
# 首选（需要 system mode — eBPF 依赖 root）
sudo anolisa install agentsight

# 备选（Alinux，需配置 YUM 源）
sudo yum install agentsight

# 源码编译（仅开发者）
cd src/agentsight && make build
```

## 快速开始

```bash
# 终端 1: 启动 eBPF 追踪（需要 root）
sudo agentsight trace

# 终端 2: 启动 Dashboard
agentsight serve
# 浏览器访问 http://localhost:7396
```

## 使用详解

### agentsight trace — 启动 eBPF 追踪

启动基于 eBPF 的内核级 AI Agent 活动捕获。

```bash
sudo agentsight trace
```

> 需要 root 权限。捕获 SSL/TLS 流量、进程事件和文件操作。

### agentsight serve — 启动 API 及 Dashboard

```bash
# 默认绑定 127.0.0.1:7396
agentsight serve

# 绑定所有接口（远程访问）
agentsight serve --host 0.0.0.0 --port 7396
```

> 远程访问时请确保防火墙已放行对应端口。

### agentsight token — 查询 Token 用量

```bash
# 今日用量
agentsight token

# 本周 vs 上周对比
agentsight token --period week --compare


# JSON 格式输出
agentsight token --json
```

### agentsight audit — 查询审计事件

```bash
# 最近的审计事件
agentsight audit

# 按 PID 和类型过滤
agentsight audit --pid 12345 --type llm

# 汇总统计
agentsight audit --summary
```

### agentsight discover — 扫描 Agent

```bash
# 发现运行中的 AI Agent
agentsight discover

# 列出已知 Agent 类型
agentsight discover --list-known
```

### agentsight interruption — 会话中断事件

查询和管理 AI Agent 会话中断事件。

**中断类型：**

| 类型 | 说明 | 默认严重级别 |
|------|------|-------------|
| `llm_error` | HTTP 状态码 >= 400 或 SSE body 包含 error | high |
| `sse_truncated` | SSE 流未收到 `finish_reason=stop` 即终止 | high |
| `context_overflow` | 上下文长度超限 | high |
| `agent_crash` | Agent 进程在会话中途消失 | critical |
| `token_limit` | `finish_reason=length` 且输出接近 max | medium |

```bash
# 列出中断事件（默认最近 24 小时）
agentsight interruption list [--last <HOURS>] [--type <TYPE>] [--severity <LEVEL>]

# 按类型统计
agentsight interruption stats

# 按严重级别统计
agentsight interruption count

# 标记为已解决
agentsight interruption resolve <ID>
```

## 配置

配置文件：`/etc/agentsight/config.json`（通过 `--config` 覆盖）。

> **重要**：用户配置文件会 **完全替换**（而非追加）内嵌的默认规则。确保配置中包含所有需要监控的 Agent 规则。

### 功能开关

| 功能 | JSON 路径 | 默认值 | 说明 |
|------|-----------|--------|------|
| Token 统计 | `features.token_stats` | `true` | 核心 Token 计账 |
| SQLite 存储 | `features.sqlite_storage.enabled` | `true` | 本地持久化 |
| 中断检测 | `features.interruption_detection.enabled` | `true` | 错误/崩溃检测 |
| 审计 | `features.audit` | `true` | LLM 调用审计 |
| Session 映射 | `features.session_mapping.enabled` | `true` | responseId→sessionId |

### 运行时资源上限

| 配置项 | 默认值 | 说明 |
|--------|--------|------|
| `event_channel_capacity` | 10,000 | Probe 事件有界通道容量 |
| `pending_genai_max_count` | 1,000 | 等待 session_id 的最大事件数 |
| `max_connection_body_mb` | 8 | 单 HTTP 连接 body 缓冲上限 |
| `ring_buffer_mb` | 32 | eBPF Ring Buffer 大小（必须为 2 的幂） |

## Agent 框架集成

### 对话式 Skill（cosh）

AgentSight 提供内置对话式 Skill，可在 Copilot Shell 中通过自然语言查询 Token 消耗和审计日志：

- 「今天 Token 用了多少？」
- 「帮我查一下今天的 LLM 调用记录」

### Token 节省（Tokenless 集成）

AgentSight 集成 Tokenless 组件的压缩统计数据，可通过 Dashboard 查看 Token 节省效果。两个组件同时安装后，节省数据自动出现在 Dashboard 中，无需额外配置。

## 数据管理

### 数据库自动限容

默认数据库最大容量：200 MB。达到上限时自动触发清理。

通过环境变量自定义：
```bash
export AGENTSIGHT_GENAI_DB_MAX_SIZE_MB=500
```

### 清理历史数据

```bash
rm -rf /var/log/sysak/.agentsight
# 然后重启 AgentSight
```

## 常见问题

**Q: 为何无法获取 OpenClaw 的 Token 消耗数据？**

A: AgentSight 监控的是 `openclaw-gateway` 守护进程。请检查客户端与 Gateway 的连接状态。若出现 "pairing required" 错误，执行 `openclaw devices approve` 完成设备配对。

**Q: 为何 Token 节省页面显示为 0？**

A: 可能原因：(1) AK/SK 认证方式暂不支持；(2) Session ID 格式非标准 UUID。

**Q: 为何累计节省量大于单次对话的即时差值？**

A: Agent 在每次对话时会将历史消息纳入上下文，因此优化收益在多轮中累积，导致累计节省量大于单次差值。
