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
| ANOLISA raw 包 | Linux x86_64，system mode |

> **macOS**：AgentSight 在 macOS 上提供 `trace`（轨迹采集器，扫描本地 JSONL 会话文件，无 eBPF）和 `serve`（Dashboard 查看器）两个命令；其余依赖 eBPF 的命令仅 Linux 可用。

## 安装

首选 ANOLISA CLI 安装已发布的组件。

```bash
# 首选（需要 system mode — eBPF 依赖 root）
sudo anolisa install agentsight

# 备选（Alinux，需配置 YUM 源）
sudo yum install agentsight

# 源码编译（仅开发者）
cd src/agentsight && make build-all
```

> 源码编译请使用 `make build-all`：它会依次构建 Dashboard 前端、主二进制和 `agentsight-enforcer`。仅执行 `make build` 不会构建 enforcer，`serve` 运行时会持续输出 `AgentSight enforcement unavailable` 日志。

## 快速开始

普通部署直接使用 systemd。这个服务会一起运行 eBPF trace 和
Dashboard，并按顺序带起 enforcer 依赖。

```bash
sudo systemctl enable --now agentsight.service
sudo systemctl status agentsight.service
```

服务进入 active 状态后，打开 `http://localhost:7396`。启用主服务后，
主机重启也会自动拉起 AgentSight。

systemd 自带的启动脚本会让 Dashboard 监听 `0.0.0.0`。主机接入不可信
网络以前，请先通过防火墙或安全组限制 7396 端口。

服务会以 root 身份和私有 umask 运行，数据保存在
`/var/log/sysak/.agentsight`。CLI 查询和 Dashboard 访问命令读取这些数据时
也要使用 `sudo`。

前台排查前先停止 systemd 服务，避免两个 tracer 同时采集。随后打开两个
终端，并以 root 身份运行两条命令。`agentsight trace` 会一直占用前台，
在同一个终端里顺序输入两条命令时，第二条命令不会开始运行。

```bash
sudo systemctl stop agentsight.service

# 终端 1
sudo agentsight trace

# 终端 2，启动 Dashboard
sudo agentsight serve
# 浏览器访问 http://localhost:7396

# 输出 Dashboard 访问地址与 Token，再用桌面用户打开
sudo agentsight dashboard --no-open
```

> 本机（localhost）访问免认证；远程访问需要携带 Token，见[Dashboard 访问与认证](#dashboard-访问与认证)。

## 使用详解

### agentsight trace — 启动 eBPF 追踪

启动基于 eBPF 的内核级 AI Agent 活动捕获。

```bash
sudo agentsight trace
```

> 需要 root 权限。捕获 SSL/TLS 流量、进程事件和文件操作。
> 启动前台 tracer 前，先执行 `sudo systemctl stop agentsight.service`。

### agentsight serve — 启动 API 及 Dashboard

```bash
# 默认绑定 127.0.0.1:7396
sudo agentsight serve

# 绑定所有接口（远程访问）
sudo agentsight serve --host 0.0.0.0 --port 7396
```

`serve` 和 `trace` 使用同一个运行用户时，才会解析到同一个数据目录。
绑定 `0.0.0.0` 会将 Dashboard 暴露给所有网卡。在使用这种方式之前，
需要先限制网络访问。

#### Dashboard 访问与认证

Dashboard Token 认证默认开启：

- **本机访问**（loopback）自动免认证，直接打开 `http://127.0.0.1:7396` 即可。
- **远程访问**需携带 Token：浏览器 URL 追加 `?token=<TOKEN>`，或 HTTP 请求头设置 `Authorization: Bearer <TOKEN>`。
- Token 在首次启动 `serve` 时自动生成（64 位十六进制），持久化于数据库同目录的 `.dashboard_token` 文件（默认 `/var/log/sysak/.agentsight/.dashboard_token`），重启后复用。
- 使用 `sudo agentsight dashboard --no-open` 输出服务生成的访问地址与 Token，再用桌面用户打开该地址。

如需关闭认证（仅建议在可信内网使用），在配置文件中设置：

```json
{
  "server": { "auth": { "enabled": false } }
}
```

### agentsight dashboard — 查看 Dashboard 访问信息

显示 Dashboard 访问地址、认证 Token，并尝试打开浏览器。在 ECS 实例上还会输出安全组放行指引。

```bash
# 输出访问地址与 Token，不以 root 身份打开浏览器
sudo agentsight dashboard --no-open
```

### agentsight summary — 统一概览

汇总最近时间窗口内的会话与 Token 用量、按严重级别统计的中断事件，以及 Tokenless 节省数据，一条命令查看整体运行状况。

```bash
# 最近 24 小时概览（默认）
agentsight summary

# 最近 7 天，JSON 格式输出
agentsight summary --last 168 --json
```

> 各数据源相互独立：某个数据库缺失时对应部分显示为 0，不影响其余输出。

### agentsight token — 查询 Token 用量

```bash
# 今日用量
sudo agentsight token

# 本周 vs 上周对比
sudo agentsight token --period week --compare

# JSON 格式输出
sudo agentsight token --json
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

# 按 ID 查看单个事件详情
agentsight interruption get <ID>

# 查看指定会话 / 对话的全部中断事件
agentsight interruption session <SESSION_ID>
agentsight interruption conversation <CONVERSATION_ID>

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
