# V2 OTel tracing 实现与验收

本工作包实现 Rust CLI → client → UDS → daemon → PAP/compiler 的 tracing，及未来
SecurityEvent/observability 消费者使用的只读关联投影。它不实现本地链路重组、事件存储、
历史查询或尚未迁移的安全 capability。架构设计见 [实现设计](V2_OTEL_IMPLEMENTATION_DESIGN_zh.md)。

本次为首次 OTel 接入。V1/V2 是产品实现版本；V1 caller 的 trace-context/metadata 输入
仍受支持，适配这些输入不表示存在旧版 OTel，也不表示接口已废弃。兼容关联标签专指
caller 提供的 opaque trace_id / invocation 标签，不包含另一套技术 tracing 身份。

## 实际落地

- `asc-observability`：一个 OTel Context 承载 SDK identity、五个 Baggage 字段和兼容标签；
  `snapshot()` 不依赖采样或 exporter。`bind_trace_context_input()` 对照冻结 V1 oracle；
  `bind_metadata(parent, value, kind)` 先按 hook 校验再替换记录字段，保留 metadata 首尾空白；
  `validate_metadata()` 区分普通调用与必填 metadata 消费者。
- `runtime` feature 仅由 CLI/daemon 入口启用，负责真实 SDK、固定未采样策略、独立 span/log
  过滤与本地诊断 worker。无公开 exporter 或 OTLP 配置；业务模块使用原生 tracing 埋点。
- 原生请求新增 version 1 `traceContext` 和 `compatibility`，严格 schema。每个 handler 请求从
  干净 context 开始，scope 覆盖授权、PAP 和响应编码。超时不提前结束仍执行中的 handler span。
- CLI 保留 V1 `--trace-context` bootstrap 位置、别名、last-wins 和错误退出码 1；新增
  `--otel-context`。client 在 child span 内注入请求副本，保持原参数、deadline 和不重试行为。
- 请求保留 4 MiB 业务容量，额外 32 KiB propagation 容量；响应仍为 4 MiB，均包含 LF。
  业务计量保留原始 whitespace/escape 字节，不通过重新序列化缩小请求。
- SDK 0.32.0 / SDK implementation 0.32.1 / bridge 0.33.0 已锁定；关闭 bridge 默认 metrics/log features。
  CLI 不启动应用级 Tokio runtime 或 HTTP client；只保留本地诊断 worker。

实现文件在 `v2/crates/data/asc-observability/src/{lib,fields,propagation,runtime}.rs`；
protocol DTO 在 `asc-daemon-protocol/src/trace.rs`。没有第二份 Agent context store 或自定义 TraceId。

## 可复现检查

从组件目录执行；真实 socket/进程测试需要运行环境允许 UDS 和子进程：

```bash
cargo fmt --all --manifest-path v2/Cargo.toml --check
cargo clippy --workspace --all-targets --manifest-path v2/Cargo.toml --locked
cargo +1.88.0 check --workspace --manifest-path v2/Cargo.toml --locked
cargo test --workspace --manifest-path v2/Cargo.toml --locked
cargo build --workspace --manifest-path v2/Cargo.toml --locked
uv run --project agent-sec-cli pytest tests/v2/e2e/test_otel_e2e.py -v
```

当前范围的 workspace Rust tests 和真实 CLI/daemon 本地日志测试已执行通过；
不保留 mock Collector 或 OTLP exporter 测试，不以历史导出测试结果作为当前交付证据。
沙箱内 socket 被拒绝，真实 UDS 验收在允许本机 socket 的环境执行。

性能不设验收要求：不比较不同机器的启动延迟、请求耗时、吞吐或峰值内存。
进程测试保留 10 s 外层 watchdog 检测挂死。
关闭预算、业务 deadline、取消后的 span 生命周期和诊断阻塞隔离仍是功能要求；
watchdog 通过不表示精确测量或证明 50 ms / 2 s 的关闭开销。

## 覆盖矩阵

以下 PASS 针对“本次已执行范围”；原设计 TO/UF 中更广的组合测试不因此自动成为全覆盖。

| 验收项 | 本次证据 | 结果与边界 |
| --- | --- | --- |
| TO-001/007/008 | `asc-observability/tests/context.rs` | PASS：native root/child、晚绑定后 contextual/explicit child、投影与恢复；交错 task/blocking、task abort 后同一 worker、blocking worker 复用、panic 恢复；metadata task 保留技术 ID/请求引用/兼容标签 |
| TO-009 | context + `asc-observability/tests/runtime.rs` | PASS：真实 runtime 固定未采样，环境 always_on/off × RUST_LOG info/off 不影响当前 ID/Baggage；未知调优设置不输出诊断；普通消息 canary 不进入关联日志 |
| TO-002/003/004/005 | context tests + `test_otel_e2e.py` | PASS（列明的正反例）：remote parent、flags/tracestate、无 parent、只带 Baggage、重复 Baggage key/非法编码/超长及成员上限、Unicode 往返；不等于完整 W3C 规范符合性 |
| TO-006 | protocol `tests/tracing.rs` + process schema case | PASS：版本、null、重复字段、未知字段，原始 UDS 拒绝且不执行业务 |
| TO-010 | writer 单测 + process blocked stderr case | PASS：队列饱和丢诊断；启动前填满 stderr 后仍完成请求、重复 daemon 启动失败和关闭，RUST_LOG info/off 均覆盖。公开 exporter 故障测试已移出范围 |
| TO-017 | process SIGTERM/blocked stderr + runtime shutdown test | PASS：正常与 stderr 阻塞时进程退出完成，stdout 无诊断污染；残留 blocking 工作不无限阻止应用 runtime 退出，不声称精确测量关闭耗时 |
| TO-011/013 | 真实非 root peer 伪造 Baggage 用例 + 原有 PAP/CLI CRUD、拒绝、revision、digest、CAS、JSON/退出码 fixtures | PASS：保持当前 V2 行为；Agent attribution 不参与 Principal 构造 |
| TO-012 | process correlation + client/context + `tracing_failures.rs` | PASS：两个真实二进制日志共享 trace、五字段和兼容标签；SDK 内存检查验证 parentage、PAP→compiler 成功/失败 span。未以本地日志声称证明跨进程每层 parent |
| TO-019 | 无 | [SUPERSEDED] 公开 OTLP 导出与 wire 验收移出本期，无 mock Collector |
| TO-014/016 | `tracing_failures.rs` + process canary + runtime tests | PASS：真实 UDS busy/read timeout/invalid envelope/unknown method/permission denied/handler panic 的内部 span 范围和错误类别；params/panic/普通消息/未知 Baggage 不进入新增关联诊断，原用户输出另按业务契约验收 |
| TO-015 | `asc-daemon/tests/tracing.rs` | PASS：真实 UDS deadline 返回后 handler span 仍打开，工作完成后闭合并记录 `cancel_requested` |
| TO-018 | client `tests/tracing.rs` + 当前原始请求 fixtures | PASS：显式 carrier 隔离、request 不变、daemon 错误只返回一次且不降级重发；旧 daemon 不属于支持范围；无 carrier 请求仍可用 |
| TO-020 | Cargo.lock + product/workspace feature trees | PASS：一套 OTel 0.32 类型；OTLP/reqwest/hyper/AWS-LC 已退出 lockfile；AgentSight Client 的 ureq/rustls/ring 保留 |
| TO-021 | protocol budget + 新增 process maximum Unicode case | PASS：4 MiB 业务帧同时携带五个各 256 个四字节字符的 Baggage，全部字段到达 daemon；业务多一个空白字节、传播预算超限分别拒绝；并发请求按 admission 功能测试验收，不做内存性能基准 |
| UF-001/002/003 | CLI bootstrap tests + frozen `v1-trace-context-normalization.json` | PASS：输入适配、别名优先级、Python 空白/Unicode 截断；实际 Agent hook 业务命令不在当前迁移范围 |
| UF-004/005/006 | context/runtime + process 未采样并发与标签用例 | PASS：opaque/invocation 标签、raw UDS root；同一 trace 的 24 个并发请求各有独立 request_span_id、开始/结束日志成对且归属一致；子 span 日志保持请求引用。范围为统一 diagnostic helper，未迁移的 V1 日志访问入口另行验收 |
| UF-010 | 原 CLI goldens + process 关闭用例 | PASS：当前 PAP stdout、默认 stderr、退出码及公开 UUID；尚未迁移的真实 hook 整体超时未宣称通过 |
| UF-011 | 冻结 `metadata.json` 的 138 个 V1 schema 输入 + context tests | PASS：AgentRun/ModelCall/ToolCall 的缺失/null/类型/别名/extra/空白/截断；带父值时仍先校验自身输入，可选字段清除；技术 parentage/agent_name/标签保留；子 span/task/carrier 投影及未采样读取。真实 observability RPC/存储仍属后续模块 |

没有以 Markdown ID 的存在替代执行。未采样时的事件持久化“独立性”在本次体现为快照可读；
不存在的 Rust SecurityEvent sink 不会被列为已经完成持久化验收。

标准语法校验仍依赖锁定的 SDK；例如 SDK 0.32 的 `TraceState` 构造会接受重复 vendor key，
本轮没有将它扩展为独立的 W3C 全规范验证器。TO-005 的通过结论仅限已列出的正反例，
不能据此宣称所有 malformed tracestate 都会被拒绝或清空。

## 兼容性与内部变更记录

| 记录 | 决策 | 对外影响 |
| --- | --- | --- |
| OTEL-CR-001 | 原生 PAP envelope 增加可选 versioned carrier | 仅支持新 CLI/new daemon；部署先升级 daemon，旧 daemon 不属于兼容门禁，不做自动重试 |
| OTEL-CR-002 | 旧 opaque trace ID 作为兼容标签 | 不转换、哈希或伪装成 OTel TraceId；V1 历史记录不在这里迁移 |
| OTEL-CR-003 | 一个 Context + Baggage + native spans | 业务函数不添加 metadata/context 参数；首期只有 PAP/compiler 埋点 |
| OTEL-CR-004 | 本期仅本地 OTel Context 与关联日志 | 取消公开 OTLP exporter、采样和 batch 配置；固定 AlwaysOff，本地 ID/Baggage 保留。有界诊断不得改变业务结果，原 CLI 结果输出保持 |
| OTEL-CR-005 | 不新增 invocation UUID 或诊断请求 ID 生成器 | 显式 invocation 标签保留；当前公开 PAP requestId UUID 保留；日志以 trace_id + request_span_id 关联 |
| OTEL-CR-006 | 独立请求 propagation 容量 | 4 MiB 业务 + 32 KiB propagation；响应不扩容；raw whitespace 不被隐藏 |
| OTEL-CR-007 | metadata 使用同一传播通道但保留自身值语义 | V1 trace-context trim；metadata 不 trim。SDK 注入会 trim，适配器以 percent encoding 保留原值 |
| OTEL-CR-008 | metadata adapter 增加 hook kind，先校验再替换记录字段 | V1 必填/null/alias/extra 规则保持；父值不掩盖缺失，省略/null 的可选字段清除。agent_name 由原 trace-context/carrier 提供；只改内部 helper，无新增 RPC 或 caller 参数 |

直接消费者：两个产品入口、同步 client、dispatcher/rejection encoder、PapService 和 compiler；
对应测试已执行。仅新增 `traceContext/compatibility` wire 字段；没有修改 PAP domain model、
revision、授权、资源 ID、事件文件、数据库或 Agent 插件配置。

## Review 修复验收

本轮 workspace Rust tests 通过；pytest E2E **8 passed，0 skipped**。
这些为本机验证，不宣称 GitHub CI job 已执行。

- 范围：移除生产 exporter/config 与 mock OTLP；保留内部 SDK span 检查、本地日志及
  V1 输入兼容 fixtures。OTEL 环境变量无法开启导出或改变固定采样策略。
- 打印审计：daemon PAP 警告、signal/runtime/bind/serve 错误和异常链使用同一有界 writer；
  OTel 初始化失败使用最多等待 50 ms 的临时 worker；均无同步 fallback。
  CLI 帮助/usage/结果/业务错误与 daemon 帮助/参数错误为必需输出，保留同步语义及背压。
- 日志：writer 单测覆盖阻塞/满队列/超长；pytest 在进程启动前填满 stderr，覆盖正常启动、
  请求拒绝和响应、队列饱和、CLI 正常退出、重复 daemon 启动失败与 SIGTERM。
  RUST_LOG info/off 均执行；不是精确延迟基准。
- 传播：context tests 覆盖未知字段非法 UTF-8 隔离、单次 header 注入、转义往返，以及
  8 KiB 内 SDK 互通和超出后仅本地 16 KiB adapter 保留的边界；不声称全 SDK 全容量互通。
- client：无可注入 SDK context 时保留显式 carrier；runtime 测试覆盖重复初始化冲突。
- 测试入口：用例位于 `tests/v2/e2e/test_otel_e2e.py`，构建二进制后由 pytest 执行；
  Makefile/CI 统一接入和 V1 测试收集边界由另一个 V2 E2E PR 处理，本变更不单独添加门禁。
  缺少 binary/socket 权限导致失败，非 root 授权用例仅在 root 环境中明确 skip，不计为通过。

内部变更：生产 exporter 配置入口退出范围；无效允许值仍丢弃整个 Baggage，但未知值不再做 UTF-8
解码；metadata/归属字段值不变，ASCII wire 转义可更简洁。JSON 诊断改为有界 best-effort 队列，
SecurityEvent 持久化契约不变。新 CLI/旧 daemon 不在支持范围，无 capability 协商或兼容降级。

生产初始化还将 Rust 默认的同步 panic hook 替换为同一有界 writer，仅输出固定
`runtime: panic`，不记录 panic payload，也不改变 unwind/abort 或业务错误映射。
内部 runtime 子进程测试验证 caught panic 的固定诊断及 payload 隔离。

## 部署与回滚

新 CLI 仅支持接入 carrier 的新 daemon。先升级 daemon，再升级 CLI/其他 native caller。回滚时先停止/回滚发送新 carrier 的 caller，
再回滚 daemon；`RUST_LOG=off` 不关闭 carrier，不能替代协议回滚。
没有状态 schema 迁移，生产没有 exporter 开关。

后续工作：Action Runtime、安全事件 sink、实际 observability RPC、AgentSight 跨服务传播、
历史查询与本地链路重组各自按其业务契约验收；本次没有把它们加入 tracing 实现范围。
