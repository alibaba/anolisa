# Reconciler 核心实现与验收记录

日期：2026-09-08。实施前历史基线为 `feat/v2-agentsight-client`
的 `4ee8b399e598108661db1686619f4791ed89e3f7`。拆分后，共享契约及 Client
适配并入 `b35e6c3a`，核心、内存 Repository 和组合测试由后续独立提交交付。

先交付的通用核心和内存原子操作现已补齐真实 AgentSight Client 适配与本地 HTTP
组合；本次新增 PAP 生命周期修正及内存组合验证，daemon 装配保持原状。验收类型为 `GREENFIELD_CONTRACT`；
不保留 V1 runtime。

## 结果矩阵

| 验证项 | 结果 | 证据 |
|---|---|---|
| 共享 Adapter/Client 契约 | PASS；3 tests | `asc-policy-target-contracts/tests/ports.rs`：完整序列化、部分结果、错误边界及不依赖核心的端口使用 |
| REC-CORE-001 至 022 | PASS；47 个变体 | `required-variants.json` 与实际 runner 清单严格相等 |
| 串行故障/重试/状态场景 | PASS；45 变体、69 步 | `core-cases.json`：完整输入/输出引用、Client 调用参数与结果、逐步状态及有序 trace |
| 线程竞争与 timeout/join | PASS；2 变体 | `concurrency.json`；两个独立 Reconciler 实例、受控 channel、真实锁竞争 |
| 核心状态决策 | PASS；9 tests | `src/state_tests.rs`：认领、预算、无确认不得成功、观察校验、运行字段与 PAP 互操作 |
| 内存 repository 依赖契约 | PASS；6 tests | `repository_contract.rs`：并发 CAS 仅一方成功、runtime/deployments 变化使旧快照失效、PAP 写入后重放、身份错误无部分更新 |
| 实际 AgentSight Adapter + fake Client | PASS；1 test | 对照已有完整 Binding/AgentSight plan golden，核心原样保存 plan/prepared |
| 实际 AgentSight Client 端口 | PASS；15 个新接口 tests | 固定 prepared-7/8 golden、进程/boot 重放、部分结果、路由验证和清理；另有 8 个 legacy tests |
| 实际 AgentSight Client 组合 | PASS；4 tests | `asc-pcp/tests/agentsight_integration.rs`：真实 Adapter/Core/Client/Ureq + loopback TCP HTTP mock |
| 现有 V2 workspace 回归 | PASS | 现有 PAP CRUD、UDS、Adapter、Client 等测试通过；不表示已接入 Reconciler |
| Clippy / format / lockfile / diff | PASS | 下列本地命令 |
| PAP 生命周期 + 内存核心 | PASS；4 tests | `pap_lifecycle.rs`：spec-only revision、不可撤销 Delete、硬删除、新 ID 创建及稳定重试 |
| 异步事件/定时器/daemon 接线 | NOT IMPLEMENTED | 下一工作块，不由同步组合 tests 替代 |
| SQL、重启恢复、真实 PEP/kernel 执行 | NOT RUN / 延期 | memory-only、真实 Client 到 HTTP mock；无 live AgentSight 或 BPF 验证 |

[ACCEPTANCE.md](ACCEPTANCE.md) 第 4 节核心矩阵、第 5 节真实组件组合及第 6 节
本地验证门禁现已完成；并不表示 PAP/daemon 触发链路或真实远端 enforcement 完成。
Client 接入由实际组合测试证明，不仅依赖 Client 独立单测。

## Review 修复增量

以下修复基于 `cadd2833`，验收为内部契约修正及 Adapter/Client conformance；
不改变 PAP 公共方法或接入 daemon worker。

| Review 项 | 处理及可执行证据 |
|---|---|
| 1：DELETE absence | code 独立解析；缺失 retryable 默认 false，429/5xx 保持 retryable；`delete_absence_depends_only_on_status_and_code` |
| 2：HTTP 凭据 | 仅 IP 字面量 loopback 允许 HTTP，其余必须 HTTPS；`bearer_credentials_require_tls_outside_literal_loopback`；localhost HTTP 配置需迁移到 127.0.0.1/::1 |
| 3：ActPlane 来源 | 按确认移除 Adapter 本地 compiler 调用及 Git 依赖，删除测试中的同版本编译断言；保留完整 DSL golden 和语义测试。lockfile 移除 7 包，无依赖升级 |
| 4：依赖审计面 | [DEPENDENCIES.md](../../DEPENDENCIES.md) 登记 TLS/unsafe 来源和发布检查；未更换 ring，也未声称已完成第三方审计或新增 CI 门禁 |
| 5、12：README | 补齐核心/共享契约清单，区分同步核心与尚未接入的调度 worker |
| 6：槽位 | 核心读取缺失 Binding 后返回 Skipped，1000 个不同缺失 ID 不产生槽位；存活记录锁身份保持；本次删除完成并确认记账后回收 registry 槽位，旧持有者仍可安全重读缺失 |
| 7：panic | `src/panic_recovery_tests.rs` 的 7 个测试、11 个故障变体通过；完整状态及 trace 检查涵盖 claim 前后、prepare/登记/目标请求、结果提交前后、存储恢复及新意图 CAS。中毒 backend/abort/daemon health 仍属明确边界 |
| 8：boot ID | 合法 UUID 写法按值比较，nil/非法 UUID 拒绝；`replay_compares_boot_uuid_values_and_preserves_request_bytes` |
| 9：daemon 一致性 | 保持未接入；核心 README 记录 DJOB/DPROC health/join 与 durable recovery 验收要求，不要求提前拆分 workspace 或引入跨进程锁 |
| 10：target 数据形状 | 详细设计第 7.0 节记录旧草案退役和当前序列化边界；不恢复无消费者的旧类型/serde envelope |
| 11：UUID feature | v5 只由 Client 请求；`cargo tree -p asc-daemon --edges normal,build,features --locked --offline` 确认无 v5/sha1_smol，也无 ureq/ring/ActPlane；workspace 构建仍允许 feature 合并 |

`cargo test --workspace --locked --offline` 在允许本地 socket 的环境重跑 **PASS**；
沙箱内首次执行的 daemon bootstrap socket 绑定失败不能作为代码回归结论。
全 workspace Clippy、format 和 diff 检查通过。该 review 时的核心 48 变体通过；本次生命周期修正改为 47 变体；
missing 变体 trace 记录一次存在性读取，不产生执行槽位。

本轮进一步将 Repository 收敛为共享层的 `get_binding_state` 和
`compare_exchange_binding_state`；内存 adapter 不再依赖 PCP。执行锁/待提交结果及
claim/register/finish 决策迁入核心，write ID 回执保证 post-commit panic 安全重放。
公开 wire 及已有完整状态序列化形状不变；trace 同步反映读取/CAS 次序。
回退时同时恢复核心、repository、port 实现和上述 fixtures；Client 配置/解析变更
可单独回退，但会重新引入 review 缺陷。当前无数据库迁移或真实 PEP 清理要求。

## 可复现命令

拆分验证：已从 `b35e6c3a` 导出独立 V2 快照（不含 `asc-pcp`），其 workspace
test、Clippy、fmt 和提交 diff 检查均通过；完整核心版本也通过同样门禁。
Client 的生产及测试依赖均不含 Reconciler/Repository。独立快照的 daemon/UDS
回归需要允许本地 socket；沙箱内启动失败后，放开测试端口限制重跑通过。

从 `v2` 执行：

```sh
cargo test -p asc-pcp --locked --offline -- --nocapture --test-threads=1
cargo test -p asc-policy-target-contracts --locked --offline
cargo test -p asc-agentsight-client --locked --offline -- --nocapture --test-threads=1
cargo test --workspace --locked --offline
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo fmt --all -- --check
git diff --check
```

核心 crate 有 39 个 Rust tests（9 个核心状态、6 个存储、9 个 panic/槽位、2 个 fixture、5 个引用解析、4 个 HTTP 组合、4 个 PAP 生命周期组合），其中一个 fixture runner 执行完整的 47 变体，
不能将 Cargo 显示的 `1 test` 误解为只有一个验收场景。
逐项输出见 [core-output.txt](core-output.txt)。完整记录位于 `objects.json`，`$ref`
允许通过语义名称递归引用共享对象，展开后仍是完整记录；禁止字段覆盖、循环、
非字符串或缺失引用，不从运行结果生成 expected。ordered trace 位于两个场景文件。runner 比较实际 trace 和最终完整
记录，出错时输出 expected/actual 差异，并拒绝未消费注入和额外调用。

Client 的独立测试输出见 [client-output.txt](client-output.txt)。迁至核心的 4 条 HTTP 组合路径覆盖：
Apply 成功和 Delete 断连重试、Update 旧目标删除成功/新目标创建失败后重试、
同 revision Delete 在 POST 返回前到达，以及 HTTP 成功后结果写库失败只重试记账。
mock server 比较确切请求方法、路径和 POST 字节；在收到 POST/DELETE 时检查
repository 中对应目标已登记 UNKNOWN，测试结束拒绝未消费的预期请求。

## Fixture 表达精简（2026-09-08）

将无语义编号替换为 `record.create.pending`、`report.replace.partial-retryable` 等
按角色/用途命名的对象。共享完整 Policy、Scope、Binding spec、目标身份和稳定请求，
保留 record 的运行字段及报告观察，不引入状态 patch 或动态生成 expected。

以下为生命周期变更前、单独精简步骤的等价性记录；当前 fixture 数量见下节。

| 文件 | 原行数 | 精简后行数 |
|---|---:|---:|
| `objects.json` | 8,626 | 2,029 |
| `core-cases.json` | 4,100 | 3,260 |
| `concurrency.json` | 270 | 165 |

三份数据文件总行数从 12,996 降至 5,454（约 58%）。46 个串行场景的 70 步及
2 个并发场景全部保留；精简前后递归展开的输入、期望记录、调用参数/结果、trace
逐项完全相等。引用解析新增共享对象展开、循环、缺失、覆盖和非字符串 5 个测试。
对象命名与维护规则见 [ACCEPTANCE.md](ACCEPTANCE.md) 第 3 节。

## 内部契约与接入注意事项

共享 trait 位于 `asc-policy-target-contracts`，数据位于 `asc-policy-types::target`。
`asc-pcp` 重导出这些名称，Client 的运行时及测试依赖均不含核心或 Repository。
HTTP 组合测试迁至 `asc-pcp/tests/agentsight_integration.rs`；原 frozen fixtures 和
HTTP 断言保持；删除成功的状态断言随本次契约改为记录不存在。共享契约及 Client 可在 Reconciler 实现提交之前独立构建测试。

实现入口：[asc-pcp](../../crates/policy/asc-pcp/README.md)。

- `asc-policy-repository` 定义共享记录和读取/完整快照 CAS；memory 与 PCP 都依赖它。
  claim/register/finish、重试计算和共享执行 slot 全部位于 PCP 内部；memory 仅做
  原子存储及写回执，与 PAP 共用唯一权威 current Binding map。
- 准备无目标修改副作用；plan 与 Client prepared 一起提交后才发修改请求。
  同步澄清 `TargetBindingPlan.content` 的 Rustdoc，未改变其 wire 格式或 Adapter API。
- 新增通用 `TargetDeploymentClient` 端口；UUID、HTTP、DSL、PID、进程重放验证
  均不属于核心。AgentSight Client 已直接实现该端口，其替换逻辑未在核心复刻。
- Client 新增固定请求的格式版本、SHA-256 digest、boot identity 及历史目标清理引用。
  `ProcessIdentityResolver::boot_id` 是必实现方法，旧直接 apply 入口及 unavailable
  默认实现已移除；固定请求路径必须提供 boot identity。`Exited` 为新增
  错误变体，生产 `/proc` resolver 将进程消失与临时读取失败区分。
- Retry policy 在首次认领时固定；相同意图后续尝试不受配置变化重置。
  新 pending 记录的空 deadline 表示立即可执行，重试返回明确的 `RetryAt`。
- 结果提交失败，待提交事务保留在共享 execution slot；下次仅重试记账，不能重复
  已完成的目标请求。存储错误与目标失败分开报告，调用方仍需错误调度和 health 接线。
- 核心是同步的单次 attempt，不自建后台线程、queue 或 timer。外部 async worker
  必须保存/join blocking handle；外层 timeout 不能被当成底层请求已结束。
- 已准入状态的竞争注入移至 tests/support，使用 aggregate CAS；memory 不再暴露
  reconcile 专属的准入方法。PAP 条件写入及生命周期语义已独立验证。

## 外部兼容性与回退

没有修改 PAP 方法、参数、返回字段、授权、UDS 或 Adapter 的现有调用签名；Client
内部 Rust API 移除旧 apply/delete，统一使用 prepared 流程，详见其
[兼容性说明](../../crates/integrations/asc-agentsight-client/README.md)。
PAP 方法/DTO 保持不变，生命周期语义按 spec-only/不可撤销删除修正；新增运行记录和错误暂未投影到公开 GET。
Cargo.lock 只新增本地 crate 及依赖关系，没有升级第三方依赖。

当前 daemon 未接核心，回退不需要迁移数据库或清理真实 PEP 对象：测试仅调用本地
HTTP mock。可先撤销核心的 Client 组合测试、内存 reconciliation 和核心实现；
若同时回退 Client 扩展，再一起撤销共享契约及其数据类型，不能遗留未解析的导入。
回退核心时同步撤销其 workspace/lockfile 接线；保留已有设计/用户改动。
若以后接入真实 PEP，必须先按保留的目标
记录清理/移交所有未确认对象，再停用核心，不能套用本轮无副作用的回退说明。

进程退出会丢失全部内存记录和未提交结果；远端迟到执行、跨重启遗留策略及分布式
fencing 均未解决。queue 去重、容量、throttle、公平性和补扫维持 TODO。

## Binding 生命周期修正（2026-09-08）

`bindingRevision` 仅在 spec 更新时 +1；Delete 与同 spec 失败重试保持 revision。
PENDING_DELETE/DELETING/DELETE_FAILED 禁止所有 UPDATE。DeleteFailed 重试重置预算，
保留 prepared/targets；全部 Absent 后以 `BindingStateWrite::delete()` 条件删除聚合。
旧 ID 的 GET/UPDATE/DELETE 为 NotFound，LIST 移除；重新部署通过 CREATE 新 ID、revision 1。

PAP Repository 写入增加 `expected: Option<&BindingView>`，Some 为 update-only，
None 为 insert-if-absent；防止 service 读到的旧记录在删除后被重建。Reconciler 保持
完整快照 CAS、观察先于完成判断及 pending completion 重放。整体删除移除回执，
其重放通过 ID 缺失确认，替换缺失 ID 一律 Conflict。执行槽位仅在删除确认且无待写
结果后回收；已有 Arc 等待者安全重读缺失。新增存储及 panic/响应故障测试覆盖这些边界。

fixture 完成删除的 expected 改为 null，删除 `REC-CORE-019/deleted`，
`REC-CORE-021/fresh-binding` 验证新 ID 的初次下发；共 47 变体。PAP 真实调用的
4 个组合测试另行验证完整删除后创建流程，原始 CAS 注入不替代 PAP 准入验收。
真实 UDS 验证同 revision Delete、Applying Delete、禁止取消删除和失败重试；
完整 CRUD 响应引用改为 `bindingPendingDelete`。

本次验证：完整 workspace 171 tests、核心 39 tests / 47 JSON 变体通过；
Clippy（-D warnings）、rustfmt、Rustdoc 与 git diff --check 通过。

验收仍是 memory-only + scripted Client / HTTP mock；没有 SQL schema 变更，
没有新增 daemon worker、跨重启恢复或真实 PEP enforcement。回退本次修正必须一起
回退状态规则、PAP 条件写接口、聚合删除语义和对应测试/文档，不能单独恢复 Delete 增版。

## 方法清理验证（2026-09-08）

删除 Client 旧直接入口、PAP status-only 写口、无消费者的 Serialization/is_terminal，
收窄核心裸 mutex/AttemptOutcome。PAP 同次构造校验、Client 批量 target 解析及测试存储
实现去重；外部请求/响应、完整 fixture 和 prepared 字节不变。

核心仍为 39 tests / 47 JSON 变体：25 个 unit tests（含内部锁/异常/fixture 验收），
14 个 integration tests。原 `tests/acceptance.rs`、`tests/panic_recovery.rs` 分别迁为
`src/acceptance_tests.rs`、`src/panic_recovery_tests.rs`，内部断言保留。
Client 34 tests（原 8 个直接入口测试已迁移）；PAP 11 个 service tests + 3 个 validation
tests；memory 2 tests；daemon PAP protocol 9 tests 通过。新增验证包括整批目标修改前
全量校验、旧 revision CAS 冲突、Delete 全状态接纳、原错误文案和编译输出拒绝后不写库。

完整 `cargo test --workspace --locked --offline`、Clippy `-D warnings`、格式与
diff 检查通过；UDS/HTTP 测试在允许本地 socket 的执行环境完成。上述仍是当前内存与
mock 组合验收，不证明真实 PEP 或跨重启恢复。内部接口回退应同步其调用方及测试。
