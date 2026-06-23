# AGENTS.md — AgentSight Navigation Map

> AI Agent 可观测性工具，基于 eBPF 捕获 LLM API 调用、Token 消耗和进程行为，无需修改 Agent 代码。

## 0. 硬性规则

### 代码
- 提交前必须运行 `cargo fmt` + `cargo clippy --all-targets -- -D warnings` + `cargo test`
- 禁止在非测试代码中使用 `unwrap()` 或 `expect()` — 用 `?`、`match` 或 `unwrap_or` 替代
- 禁止使用 `dbg!()` — 用 `log::debug!()` 或 `tracing::debug!()` 替代
- 禁止添加 `#[allow(clippy::...)]` 除非附带注释说明原因
- PR diff 不应超过 800 行；复杂逻辑变更应控制在 500 行以内，超出则拆分为多个可 review 的阶段

### 架构
- 禁止高层模块直接 import 低层模块（如 `server/` → `probes/`），遵循 [ARCHITECTURE.md](docs/ARCHITECTURE.md) 中 L0–L8 层级约束
- 优先扩展现有模块，而非创建新文件
- 单模块目标 < 500 行（不含测试）；超过 2,000 行的文件在增加代码前必须先有拆分计划

### 测试
- 流水线逻辑变更（probes → parser → aggregator → analyzer → genai → storage）必须包含集成测试
- 跨模块行为优先写集成测试，而非单元测试
- 测试代码放在独立的 `*_tests.rs` 文件或 `#[cfg(test)] mod tests` 中，避免在主实现中添加仅测试用的函数

### FFI
- 修改 FFI 函数签名时必须同步更新 `cbindgen.toml` 并确认 `build.rs` drift guard 通过
- FFI 类型必须使用 `#[repr(C)]`
- 禁止 panic 穿越 FFI 边界 — 使用 `std::panic::catch_unwind`

### eBPF
- 禁止修改 BPF 程序而不验证 kernel >= 5.8 兼容性
- BPF 变更必须在真实内核上测试，仅编译通过不够

## 1. Quick Start

```bash
make build-all          # 构建前端+Rust二进制
sudo agentsight trace   # 启动 eBPF 追踪
agentsight serve        # 启动 API 服务器 + Dashboard UI（http://127.0.0.1:7396）
```

详见 → [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)

## 2. Architecture

数据流水线：`Probes → Parser → Aggregator → Analyzer → GenAI → Storage`

```
eBPF Probes → Event → Parser → ParsedMessage → Aggregator → AggregatedResult
                 ↓                                                  ↓
           ProcMon/SSL                                     Analyzer → AnalysisResult
                                                                    ↓
                                              GenAIBuilder → GenAISemanticEvent → Exporter
                                                                    ↓
                                                              Storage (SQLite/SLS)
```

详见 → [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)

## 3. 代码表面增长控制（Footprint Ladder）

新功能必须选择能解决问题的**最高级别**（最少代码表面）。只有当高级别确实无法实现时，才降级到下一级。

| 级别 | 手段 | 新增代码表面 | 何时使用 | 示例 |
|------|------|------------|---------|------|
| **1** | 扩展现有函数/方法 | 最少 | 功能可通过修改现有逻辑实现 | 在 `AgentScanner` 中新增一个 Agent 匹配规则 |
| **2** | 模块内新增 helper 函数 | 少 | 需要复用逻辑但不改变模块接口 | 在 `parser/sse/` 中提取内部解析 helper |
| **3** | 新增模块文件 | 中 | 职责明确独立，现有模块无法容纳 | 新建 `src/interruption/` 模块 |
| **4** | 新增 eBPF 探针 | 大 | 需要捕获新的内核/用户态事件 | 新增 `src/bpf/gotls.bpf.c` + Rust wrapper |
| **5** | 新增 `extern "C"` FFI 导出 | 最大 | 需要暴露新能力给 C 调用方 | 新增 FFI 函数（须同步 cbindgen.toml + drift guard） |

**规则**：
- 从级别 1 开始评估，逐级下降，在 PR 描述中说明为什么当前级别不够
- 禁止直接跳到级别 3-5 而不先考虑是否可以扩展现有代码
- 级别 4-5 的变更必须在 PR 中附带架构影响说明

## 4. Module Map

| 模块 | 位置 | 职责 | 关键类型 |
|------|------|------|----------|
| **Probes** | `src/probes/` | eBPF 探针管理 | `Probes`, `ProbesPoller`, `SslSniff`, `ProcMon`, `FileWatch`, `FileWriteProbe`, `UdpDns`, `TcpSniff` |
| **Event** | `src/event.rs` | 统一事件枚举 | `Event::{Ssl, Proc, ProcMon, FileWatch, FileWrite, UdpDns}` |
| **Parser** | `src/parser/` | 协议解析（HTTP/1.x, HTTP/2, SSE, ProcTrace） | `Parser`, `ParsedMessage` |
| **Aggregator** | `src/aggregator/` | 请求-响应关联 | `Aggregator`, `AggregatedResult` |
| **Analyzer** | `src/analyzer/` | Token/审计/消息分析 | `Analyzer`, `AnalysisResult` |
| **GenAI** | `src/genai/` | 语义事件构建+导出 | `GenAIBuilder`, `GenAISemanticEvent`, `GenAIExporter` |
| **Storage** | `src/storage/` | SQLite 持久化 | `Storage`, `SqliteStore`, `AuditStore`, `TokenStore` |
| **Discovery** | `src/discovery/` | Agent 进程发现 | `AgentScanner`, `AgentMatcher`, `known_agents` |
| **Health** | `src/health/` | Agent 健康检查 | `HealthChecker`, `HealthStore` |
| **Tokenizer** | `src/tokenizer/` | LLM Token 计数 | `LlmTokenizer`, `MultiModelTokenizer` |
| **ATIF** | `src/atif/` | 轨迹格式导出 | `AtifDocument`, `convert_trace_to_atif` |
| **Server** | `src/server/` | HTTP API + 嵌入式前端 | `AppState`, `run_server` |
| **Config** | `src/config.rs` | 统一配置 | `AgentsightConfig` |
| **Unified** | `src/unified.rs` | 主编排器 | `AgentSight` |

## 5. Critical Code Paths

1. **SSL 捕获流程**: `sslsniff.bpf.c` → `Probes::run()` → `Event::Ssl` → `Parser::parse_ssl_event()` → `HttpConnectionAggregator` → `Analyzer::analyze_aggregated()` → `Storage::store()`
2. **Agent 自动发现**: `procmon.bpf.c` → `Event::ProcMon::Exec` → `AgentSight::handle_procmon_event()` → `AgentScanner::on_process_create()` → `Probes::attach_process()`
3. **Token 提取**: `SSE Parser` → `TokenParser::parse_event()` → `TokenRecord` → `TokenStore::add()`
4. **GenAI 语义构建**: `AnalysisResult` → `GenAIBuilder::build()` → `GenAISemanticEvent::LLMCall` → `GenAIExporter::export()`

## 6. eBPF Probes

| 探针 | BPF 程序 | 功能 |
|------|----------|------|
| sslsniff | `src/bpf/sslsniff.bpf.c` | uprobe on SSL_read/SSL_write 捕获加密流量明文 |
| proctrace | `src/bpf/proctrace.bpf.c` | tracepoint on execve 捕获命令行参数 |
| procmon | `src/bpf/procmon.bpf.c` | 进程创建/退出事件（Agent 发现） |
| filewatch | `src/bpf/filewatch.bpf.c` | 监控 .jsonl 文件打开事件 |
| filewrite | `src/bpf/filewrite.bpf.c` | fentry on vfs_write 捕获 .jsonl 写入内容 |
| udpdns | `src/bpf/udpdns.bpf.c` | fentry on udp_sendmsg 捕获 DNS 查询（域名→IP）|
| tcpsniff | `src/bpf/tcpsniff.bpf.c` | fentry on tcp_recvmsg/sendmsg 捕获明文 HTTP 流量 |

构建时 `build.rs` 通过 `libbpf-cargo` 自动生成 eBPF skeleton。

## 7. CLI Subcommands

| 命令 | 入口 | 功能 |
|------|------|------|
| `agentsight trace` | `src/bin/cli/trace.rs` | eBPF 追踪（需 root） |
| `agentsight serve` | `src/bin/cli/serve.rs` | API + Dashboard 服务器 |
| `agentsight token` | `src/bin/cli/token.rs` | 查询 Token 消耗 |
| `agentsight audit` | `src/bin/cli/audit.rs` | 查询审计事件 |
| `agentsight discover` | `src/bin/cli/discover.rs` | 发现运行中的 AI Agent |
| `agentsight metrics` | `src/bin/cli/metrics.rs` | Prometheus 格式指标 |
| `agentsight interruption` | `src/bin/cli/interruption.rs` | 查询/管理会话中断事件 |

### 6.1 Interruption CLI 详细用法

查询和管理 AI Agent 会话中断事件。数据存储于 SQLite 数据库。

**数据库路径**: `/var/log/sysak/.agentsight/interruption_events.db`（可通过 `--db` 覆盖）

**中断类型**:

| 类型 | 含义 | 默认严重级别 |
|------|------|-------------|
| `llm_error` | HTTP 状态码 >= 400 或 SSE body 包含 `{"error":...}` | high |
| `sse_truncated` | SSE 流未收到 `finish_reason=stop` 即终止 | high |
| `context_overflow` | 上下文长度超限（`context_length_exceeded`） | high |
| `agent_crash` | Agent 进程在会话中途消失（OOM/signal） | critical |
| `token_limit` | `finish_reason=length` 且 `output_tokens >= max_tokens * 0.95` | medium |

**严重级别**: `critical` > `high` > `medium` > `low`

**子命令**:

```bash
# 列出中断事件（默认最近24小时，最多100条）
agentsight interruption list [--last <HOURS>] [--type <TYPE>] [--severity <LEVEL>] [--agent <NAME>] [--unresolved|--resolved] [--limit <N>] [--json]

# 按类型统计中断数量
agentsight interruption stats [--last <HOURS>] [--json]

# 按严重级别统计未解决的中断数量
agentsight interruption count [--last <HOURS>] [--json]

# 获取单个中断事件详情
agentsight interruption get <INTERRUPTION_ID> [--json]

# 列出指定 session 的所有中断
agentsight interruption session <SESSION_ID> [--json]

# 列出指定 conversation 的所有中断
agentsight interruption conversation <CONVERSATION_ID> [--json]

# 标记中断为已解决
agentsight interruption resolve <INTERRUPTION_ID>

# 使用自定义数据库路径
agentsight interruption --db /path/to/interruption_events.db list --last 48
```

## 8. API Endpoints

| 路径 | 方法 | 功能 |
|------|------|------|
| `/health` | GET | 健康检查 |
| `/metrics` | GET | Prometheus token 指标 |
| `/api/sessions` | GET | 会话列表 |
| `/api/sessions/{id}/traces` | GET | 会话下的 trace |
| `/api/traces/{id}` | GET | trace 详情 |
| `/api/conversations/{id}` | GET | conversation 事件详情 |
| `/api/agent-names` | GET | Agent 名称列表 |
| `/api/timeseries` | GET | 时序 Token 统计 |
| `/api/agent-health` | GET | Agent 健康状态 |
| `/api/agent-health/{pid}` | DELETE | 删除健康条目 |
| `/api/agent-health/{pid}/restart` | POST | 重启 Agent |
| `/api/export/atif/trace/{id}` | GET | ATIF trace 导出 |
| `/api/export/atif/session/{id}` | GET | ATIF session 导出 |
| `/api/export/atif/conversation/{id}` | GET | ATIF conversation 导出 |
| `/api/token-savings` | GET | Token 节省统计（`start_ns`, `end_ns`, `agent_name`） |
| `/api/interruptions` | GET | 中断事件列表（`start_ns`, `end_ns`, `agent_name`, `type`, `severity`, `resolved`, `limit`） |
| `/api/interruptions/count` | GET | 中断计数按严重级别（`start_ns`, `end_ns`） |
| `/api/interruptions/stats` | GET | 中断按类型统计（`start_ns`, `end_ns`） |
| `/api/interruptions/session-counts` | GET | 按 session 分组的中断计数（`start_ns`, `end_ns`） |
| `/api/interruptions/conversation-counts` | GET | 按 conversation 分组的中断计数（`start_ns`, `end_ns`） |
| `/api/interruptions/{id}` | GET | 单个中断事件详情 |
| `/api/interruptions/{id}/resolve` | POST | 标记中断为已解决 |
| `/api/sessions/{id}/interruptions` | GET | 指定 session 的所有中断 |
| `/api/conversations/{id}/interruptions` | GET | 指定 conversation 的所有中断 |

## 9. Frontend

React + TypeScript + Webpack + Tailwind CSS，位于 `dashboard/`。开发: `npm run dev`(localhost:3004)，嵌入构建: `npm run build:embed`。

## 10. Configuration

`AgentsightConfig`（`src/config.rs`），关键环境变量：SLS_*（阿里云日志服务导出）、`AGENTSIGHT_TOKENIZER_PATH`、`AGENTSIGHT_CHROME_TRACE`、`RUST_LOG`。

### 配置文件加载语义

Agent 规则配置文件路径：`/etc/agentsight/config.json`（可通过 `--config` 覆盖），格式参见项目根目录 `agentsight.json`。

**重要：用户配置文件会完全替换（replace）内嵌的默认规则，而非追加（extend）。** 如果配置文件中缺少某个 Agent 的规则（如 `*claude*`），该 Agent 将不会被发现。修改配置前请确保包含所有需要监控的 Agent 规则。

## 11. Design Docs

- [eBPF Probes 设计](docs/design-docs/ebpf-probes.md)
- [数据流水线设计](docs/design-docs/data-pipeline.md)
- [GenAI 语义层设计](docs/design-docs/genai-semantic.md)
- [常见踩坑记录](docs/PITFALLS.md) — AI agent 和新贡献者最容易踩的坑
- [架构决策记录（ADR）](docs/adr/) — 关键架构选型的背景和理由

## 12. Scoped Rules

高风险模块有独立的边界约束文件：

| 模块 | 规则文件 | 关注点 |
|------|----------|--------|
| FFI 导出层 | [src/FFI_AGENTS.md](src/FFI_AGENTS.md) | ABI 安全、cbindgen 同步、panic 隔离 |
| 主编排器 | [src/UNIFIED_AGENTS.md](src/UNIFIED_AGENTS.md) | 禁止业务逻辑、保持委托模式 |
| 存储层 | [src/storage/AGENTS.md](src/storage/AGENTS.md) | SQL 注入防护、schema 兼容、mutex 处理 |

## 13. Prerequisites

- Linux kernel >= 5.8（BTF 支持）
- Rust >= 1.80
- clang/llvm >= 11（eBPF 编译）
- libbpf >= 0.8
