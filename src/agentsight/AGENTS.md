# AGENTS.md — AgentSight Navigation Map

> AI Agent 可观测性工具，基于 eBPF 捕获 LLM API 调用、Token 消耗和进程行为，无需修改 Agent 代码。

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

## 3. Module Map

| 模块 | 位置 | 职责 | 关键类型 |
|------|------|------|----------|
| **Probes** | `src/probes/` | eBPF 探针管理 | `Probes`, `ProbesPoller`, `SslSniff`, `ProcMon`, `FileWatch` |
| **Event** | `src/event.rs` | 统一事件枚举 | `Event::{Ssl, Proc, ProcMon, FileWatch}` |
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

## 4. Critical Code Paths

1. **SSL 捕获流程**: `sslsniff.bpf.c` → `Probes::run()` → `Event::Ssl` → `Parser::parse_ssl_event()` → `HttpConnectionAggregator` → `Analyzer::analyze_aggregated()` → `Storage::store()`
2. **Agent 自动发现**: `procmon.bpf.c` → `Event::ProcMon::Exec` → `AgentSight::handle_procmon_event()` → `AgentScanner::on_process_create()` → `Probes::attach_process()`
3. **Token 提取**: `SSE Parser` → `TokenParser::parse_event()` → `TokenRecord` → `TokenStore::add()`
4. **GenAI 语义构建**: `AnalysisResult` → `GenAIBuilder::build()` → `GenAISemanticEvent::LLMCall` → `GenAIExporter::export()`

## 5. eBPF Probes

| 探针 | BPF 程序 | 功能 |
|------|----------|------|
| sslsniff | `src/bpf/sslsniff.bpf.c` | uprobe on SSL_read/SSL_write 捕获加密流量明文 |
| proctrace | `src/bpf/proctrace.bpf.c` | tracepoint on execve 捕获命令行参数 |
| procmon | `src/bpf/procmon.bpf.c` | 进程创建/退出事件（Agent 发现） |
| filewatch | `src/bpf/filewatch.bpf.c` | 监控 .jsonl 文件打开事件 |

构建时 `build.rs` 通过 `libbpf-cargo` 自动生成 eBPF skeleton。

## 6. CLI Subcommands

| 命令 | 入口 | 功能 |
|------|------|------|
| `agentsight trace` | `src/bin/cli/trace.rs` | eBPF 追踪（需 root） |
| `agentsight serve` | `src/bin/cli/serve.rs` | API + Dashboard 服务器 |
| `agentsight token` | `src/bin/cli/token.rs` | 查询 Token 消耗 |
| `agentsight audit` | `src/bin/cli/audit.rs` | 查询审计事件 |
| `agentsight discover` | `src/bin/cli/discover.rs` | 发现运行中的 AI Agent |
| `agentsight metrics` | `src/bin/cli/metrics.rs` | Prometheus 格式指标 |

## 7. API Endpoints

| 路径 | 方法 | 功能 |
|------|------|------|
| `/health` | GET | 健康检查 |
| `/metrics` | GET | Prometheus token 指标 |
| `/api/sessions` | GET | 会话列表 |
| `/api/sessions/{id}/traces` | GET | 会话下的 trace |
| `/api/traces/{id}` | GET | trace 详情 |
| `/api/agent-names` | GET | Agent 名称列表 |
| `/api/timeseries` | GET | 时序 Token 统计 |
| `/api/agent-health` | GET | Agent 健康状态 |
| `/api/agent-health/{pid}` | DELETE | 删除健康条目 |
| `/api/agent-health/{pid}/restart` | POST | 重启 Agent |
| `/api/export/atif/trace/{id}` | GET | ATIF trace 导出 |
| `/api/export/atif/session/{id}` | GET | ATIF session 导出 |

## 8. Frontend

React + TypeScript + Webpack + Tailwind CSS，位于 `dashboard/`。开发: `npm run dev`(localhost:3004)，嵌入构建: `npm run build:embed`。

## 9. Configuration

`AgentsightConfig`（`src/config.rs`），关键环境变量：SLS_*（阿里云日志服务导出）、`AGENTSIGHT_TOKENIZER_PATH`、`AGENTSIGHT_CHROME_TRACE`、`RUST_LOG`。

## 10. Design Docs

- [eBPF Probes 设计](docs/design-docs/ebpf-probes.md)
- [数据流水线设计](docs/design-docs/data-pipeline.md)
- [GenAI 语义层设计](docs/design-docs/genai-semantic.md)

## 11. Prerequisites

- Linux kernel >= 5.8（BTF 支持）
- Rust >= 1.80
- clang/llvm >= 11（eBPF 编译）
- libbpf >= 0.8
