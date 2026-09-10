# 首版 Binding Reconciler 验收标准

状态：`[TARGET V2]`，核心矩阵、真实 Adapter/Client/HTTP transport 组合及本地
验证门禁已通过。远端为 loopback HTTP mock，并非真实 AgentSight 或 kernel。
实现与证据见 [执行报告](RESULTS.md)；PAP/daemon 接线仍是后续独立工作块。

设计依据：[详细方案](../../../docs/design/BINDING_RECONCILER_DESIGN_AND_IMPLEMENTATION_zh.md)
及[讨论记录](../../../docs/design/BINDING_RECONCILER_DECISIONS_zh.md)。
本文是首版 **Reconciler 核心**的必过范围；详细方案的跨组件矩阵是全链路索引，
不能将其中的延期能力重新作为核心验收前置条件。这里的“保存/提交”指内存
repository 的原子操作，不代表 SQL 或磁盘耐久性。

## 1. 验收对象与边界

验收对象是实际 Reconciler 实现、它的状态/部署记录操作及有界重试逻辑。
测试直接调用 `reconcile(binding_id)` 或实现中等价的核心入口，不要求 daemon/UDS。
使用实现中的内存 repository；故障与竞争通过 wrapper 注入，不能另写一份假的
Reconciler 或只测状态枚举替代实际执行。

| 层次 | 本标准如何验证 | 归属 |
|---|---|---|
| Reconciler | 完整内存记录、fake Adapter/Client、可控时钟和同步点；执行真实核心逻辑 | 本文全部必过 |
| 内存 repository | 条件检查和更新在同一临界区；新请求不覆盖部署记录；结果更新保留新意图 | Reconciler 的直接依赖，必须有对应测试 |
| AgentSight Adapter/Client | 接口实现编译接入，运行各自单测；具体 DSL/HTTP/UUIDv5/进程身份语义归各自测试 | 具体组件接入证据，不混入通用核心断言 |
| PAP API | 请求增版、准入、幂等、完整返回及 wire fixture | PAP 改动块 |
| PAP 与 Reconciler 交互 | 请求提交后的通知、实际后台执行、GET 结果、已有 shutdown 接线 | 交互块；不阻碍核心独立验收 |

以下均不作为本标准的前置条件：SQL repository、数据库迁移、进程重启后的状态
恢复、补扫、job queue 去重/容量/throttle/公平性、informer、通用 scheduler、
多 PEP 实现、跨服务执行顺序协议、真实 AgentSight 服务及 BPF/kernel enforcement。
内存进程退出后记录会丢失，PEP 对象可能遗留，此限制不得写成已解决。

## 2. 固定判定规则

1. Reconciler 重读最新 Binding；不按通知中缓存的旧 spec/命令执行。
2. 同 Binding 的锁覆盖读库认领、Client 调用、结果记账及收尾。没有两个本地目标
   操作重叠；上一任务未结束或 join 前不能让下一任务开始目标调用。
3. 认领和生命周期更新原子检查 Binding ID、expected revision 与 expected status。
   用户 Delete 不增版；旧 Apply 不能覆盖新的 `PENDING_DELETE`。
4. 创建/更新前先保存 Client 返回的目标身份、稳定 prepared 和 UNKNOWN 记录；
   保存失败则零目标修改调用。prepared 是不透明内容，不由 Reconciler 重建。
5. Client 返回后，先保存目标观察，再按 CAS 推进生命周期；可以使用一个原子结果
   操作完成。旧任务仍可保存自己产生的目标事实，但不能覆盖新意图及其重试/错误。
6. 只有明确确认 Absent 才回收记录。失败、超时、无明确确认和预算耗尽都保留
   清理责任；`UNKNOWN` 不等同于“没有部署”。
7. READY 要求 Client 确认本次期望及旧目标清理完成。Delete 只有在所有目标确认
   Absent 且原意图仍匹配时，才原子移除整个 Binding 聚合；不保留 DELETED 行。
8. revision 只随 spec 变化。删除不可撤销；删除后重新部署由 PAP CREATE 新 ID、
   revision 1，核心不自行分配版本或 PEP ID。
9. 同次重试使用同一 prepared；重复通知不绕过退避或预算。重试耗尽进入对应
   FAILED，记录仍保留；FAILED 不因重复触发自动开始新预算。
10. 新 Delete 不继承旧 Apply 的预算或退避。PAP 对新意图的预算初始化由 repository
    契约测试覆盖；核心根据已接受的最新状态执行。

重试 fixture 固定输入 `max_attempts=3`（含首次）、`base_delay=100ms`、
`max_delay=150ms`，不加 jitter。第 1、2 次可重试失败后分别等待 100ms、150ms；
第 3 次失败写 FAILED，nextAttemptAt 为空。这是可执行样例参数，不冻结产品默认值。
只推进虚拟时钟，不使用真实 sleep 证明时序。一次认领消耗一次预算，重复通知和
尚未到期的调用不消耗；未成功认领不能修改预算或产生目标副作用。

Adapter 语义拒绝、Client/prepare 明确拒绝进入 Apply/Delete 对应失败状态。
AdapterFault 作为内部错误以安全 code 记录并按有界重试处理；Client/prepare 的
可重试错误沿用其分类。repository 错误单独报告，不能冒充远端拒绝或成功。
具体 Rust 错误类型和 disposition 名称可以变化，以下可观察判定不能变化。

## 3. Fixture 与 runner 的最低交付要求

每个场景必须有完整 JSON fixture 或完整序列化数据引用，并由 crate-local runner
执行。以下是必须承载的内容，不是已经实现的 schema：

| 内容 | 要求 |
|---|---|
| `caseId` / variant | 对应第 4 节 ID；同一行要求的分支均有单独变体，不能只跑其中一个 |
| initial records | 完整 BindingView/spec/IR/digest、部署记录、prepared、重试和错误；空集合显式给出 |
| trigger sequence | 核心调用、已接受意图的注入、时钟推进、竞争同步点及故障注入 |
| dependency results | Adapter 完整输出、Client prepared/目标身份/逐目标结果、repository 故障 |
| expected records | 完整最终 Binding 与运行记录；不能仅检查 status 或记录数量 |
| expected disposition | 成功、跳过、被覆盖、等待重试、最终失败或存储错误及安全 code |
| expected trace | 可检查的有序调用、参数和结果，包括未发生调用的断言 |

共同数据通过 `objects.json` 中的语义名称引用复用，执行前递归展开成完整记录。
允许对象引用其它共享对象；引用节点只能含 `$ref`，禁止覆盖字段、循环引用、
非字符串引用和缺失引用。禁止用省略字段或临时手拼不完整 Binding 绕过边界。动态身份可以符号绑定；不要求所有
PEP 身份都是 UUID。prepared 内容以字节或等价的无损编码比较，不经核心 JSON 改写。

对象名按数据角色和状态命名，禁止使用 `object-001` 一类无语义编号：

| 前缀 / 示例 | 含义 |
|---|---|
| `policy.prevent-file-deletion`、`scope.pid-4242.rev-3` | 共享的完整 Policy 和 Scope |
| `binding.rev-7`、`binding.rev-8` | 引用 Policy/Scope 的完整 Binding spec；数字表示真实 revision |
| `target.old-a`、`target.new-b` | A 是旧部署，B 是本次 Apply 的候选部署；不是 policy 类型 |
| `plan.opaque`、`prepared.new-b`、`saved.create-new-b.rev-7` | Adapter plan、Client 固定请求、带来源 revision/更新模式的保存记录 |
| `prepare.new-b.ok`、`request.new-b.no-previous`、`report.new-b.present` | Client 准备结果、调用输入、目标观察报告；无 previous 的输入也可用于部分清理后的 update 重试 |
| `record.create.pending`、`record.replace.partial-retry` | 完整运行状态快照，以操作和状态描述用途 |
| `record.skip.ready`、`record.skip.applying` | 专门验证跳过逻辑的既有状态输入，不代表完整执行成功后的输出 |

record 中的 status、attempts、deadline、error、deployment presence 保持显式可见；
共享稳定 spec 和请求不需要为每种运行状态复制 Policy/IR。expected 独立声明，
不得从运行结果生成或由 Reconciler 的状态转换函数推导。新增 policy 的翻译样例
归属 Adapter；核心矩阵仅在增加执行/记账语义时扩展，不按 policy 类型复制。

trace 至少记录 read、claim 成功/冲突、Adapter、prepare、目标登记提交、Client
调用/返回、目标结果提交、生命周期 CAS 结果及重试决定。应验证的顺序例如：

```text
claim → Adapter → prepare → 保存目标成功 → create/update → 保存结果 → 完成状态
```

观察与状态同事务提交时，记录事务结果及 CAS 判定即可，不强制实现两次事务。
核心 trace 只记录 `Client.update`，不要求它记录或编排 Client 内部 DELETE/POST。
竞争测试用 barrier/channel 等可控同步点，比较同 Binding 顺序和必要的先后关系，
不依赖不同 Binding 的全局线程调度顺序。测试超时可用作死锁保护，不用作竞争证明。

runner 必须在以下情况失败：缺失/重复 case、缺失预期变体、fixture 未完整解码、
未消费的故障/响应、额外目标调用、错误参数或顺序、prepared 变更、最终记录差异。
每次运行输出 case/variant 的 expected 与 actual 差异，不允许静默跳过失败用例。

## 4. 首版核心必过矩阵

以下 22 项及各自要求的变体均为必过。当前核心矩阵的 47 个变体已 **PASS**，
清单见 [required-variants.json](required-variants.json)，运行证据见 [执行报告](RESULTS.md)。

| ID | 场景与必要变体 | 通过条件 |
|---|---|---|
| REC-CORE-001 | 初次 Apply 成功 | Adapter 收到完整 spec；prepare 返回身份/内容被原样保存；登记先于 create；结果保存后 READY |
| REC-CORE-002 | 已有部署更新成功；不同目标 ID / 复用目标 ID 两个 fake Client 变体 | 调用 update，传入正确记录和 prepared；不直接拆成 delete+create；本次目标不被误列为旧清理对象；READY 与 Client 确认一致 |
| REC-CORE-003 | A 已生效，B 意图随后被 Delete 覆盖且 B 未执行 | 清理保留的 A；不只根据当前 revision 猜目标；不调用 Adapter/prepare/create/update |
| REC-CORE-004 | 正常多目标 Delete；空目标 Delete | 非空清理全部未确认 Absent 的记录；全部确认后聚合不存在；空集合仅在无旧本地执行且登记不变量成立时直接完成 |
| REC-CORE-005 | Apply 执行中同 revision 接受 Delete；旧任务成功/可重试失败/永久失败三变体 | 旧结果可记账，生命周期 CAS 失败；PENDING_DELETE 及新预算/错误不被覆盖；旧调用退出后再次调用核心能完成 Delete |
| REC-CORE-006 | 同 Binding 两次核心调用竞争；首次结果写回被阻塞 | 至多一次认领及目标调用在执行；锁直到结果处理完成；后续重读最新状态，无重复副作用 |
| REC-CORE-007 | 认领后 revision 改变 / 仅 status 改变 | 两种 CAS 均拒绝旧生命周期写入；只合并已登记目标的事实；不写回整份旧 BindingState |
| REC-CORE-008 | 首次读取失败 / claim 存储失败 / claim 冲突 | 零目标修改调用；未成功 claim 不消耗预算；冲突与存储错误可区分 |
| REC-CORE-009 | prepared/目标登记保存失败 | create/update 均未调用；旧目标保留；返回存储错误，不虚报 READY |
| REC-CORE-010 | Client 成功后结果保存失败及随后恢复写入 | 原子结果未部分提交；不报告完成、不丢预登记目标；进程内重试记账不再次发送已完成请求 |
| REC-CORE-011 | Adapter 语义拒绝 | 不调用 prepare 或目标修改接口；APPLY_FAILED 与安全拒绝 code 正确；旧目标保留 |
| REC-CORE-012 | AdapterFault | 不调用目标修改接口；安全内部 code；按有界预算退避/耗尽，不当成成功或策略拒绝 |
| REC-CORE-013 | prepare 可重试错误 / 明确拒绝 | 零目标修改调用；对应退避或 APPLY_FAILED；不虚构新目标存在或删除旧记录 |
| REC-CORE-014 | create 返回结果未知，随后重试成功 | UNKNOWN 保留；退避后复用同一目标和 prepared，不重新准备不同身份；成功记账后 READY |
| REC-CORE-015 | update 部分成功：旧 A 确认 Absent，新 B Unknown 或被拒绝 | 仅 A 可回收；B 记录保留；整体不写 READY；按分类重试或 APPLY_FAILED，不自动回滚 A |
| REC-CORE-016 | Delete 部分成功；可重试/永久失败/耗尽三个变体 | 仅明确 Absent 记录可回收，其余保留；重试只清理剩余目标；未确认目标保留时不能移除 Binding |
| REC-CORE-017 | Apply 与 Delete 分别连续可重试失败 | 按第 2 节精确比较 3 次预算及 100/150ms 退避；到期前无调用；耗尽后 FAILED、nextAttemptAt 为空且保留目标 |
| REC-CORE-018 | Apply 退避中接受新 Delete | 最新 Delete 立即可认领，不等旧退避、不继承旧次数；目标记录保留并用于清理 |
| REC-CORE-019 | 缺失 Binding、各终态、未到期 pending、旧通知 | 缺失/终态/未到期不发目标请求，不重置记录和预算；旧通知重读库；到期执行不超过该轮允许次数 |
| REC-CORE-020 | 两个 fake Client：非 UUID 身份、直接复用 SecCore ID；不透明 prepared 含非 JSON 字节 | 相同核心无需 PEP 分支或 UUIDv5；身份/内容原样保存回传；旧记录仍按其 target 路由，不按当前配置重写；无法解析目标时保留记录并报错 |
| REC-CORE-021 | 新 ID、revision 1 的 Binding | 核心为新 Binding 调用 prepare 并保存新产物；PAP 生命周期组合测试另行验证先删除再 CREATE |
| REC-CORE-022 | Client 调用被阻塞时请求停止/超时并安排同 Binding 下一任务 | 不能丢弃仍执行的本地调用后释放锁；前一执行及结果处理完整退出后才允许下一目标调用；不要求测试完整 daemon lifecycle |

核心 fixture 通过原始 aggregate CAS 注入状态，验证结果隔离；REC-CORE-007/revision
使用合成的延迟完成，不能解释为 PAP 允许执行中 changed-spec UPDATE。
PAP 准入由 `pap_service.rs`、`pap_lifecycle.rs` 及真实 UDS 测试固定，删除侧禁止返回
Apply。fixture 中的旧观察是最后确认事实，不是远端实时状态。

### Review 后补充的必过边界

当前 22 项/47 变体；REC-CORE-019/missing 的 trace 为一次 read：核心确认缺失
后不分配槽位。首次执行增加槽位分配前的存在性读取；登记/完成在 CAS 前读取完整
快照，完成 CAS 冲突会重读并合并。trace 中 claim/register/finish 是 wrapper 按写入
前后状态标注的 CAS 阶段，不是 repository 方法；matched 表示 CAS 成功，旧任务的
最终 Disposition 仍可为 Superseded。

| 场景 | 可执行证据与判据 |
|---|---|
| 未知 ID 槽位增长 | 核心 `src/panic_recovery_tests.rs::unknown_ids_do_not_allocate_execution_slots`：1000 个不同 ID 后槽位仍为空 |
| 同 Binding 锁身份 | `src/state_tests.rs`、`src/panic_recovery_tests.rs`：不同实例共享锁；存活记录/新 Delete 保留相同 Arc；物理删除确认后回收 registry 槽位，旧等待者重读缺失；blocking/join fixture 继续通过 |
| panic 收尾 | `src/panic_recovery_tests.rs`：claim 前后、prepare、登记、create/delete、完成事务前后注入；完整 Binding fixture 驱动，检查结果与有序执行；panic 向拥有者传播，执行槽位不中毒 |
| 失败结果保存/新意图 | 同上：存储失败只重试记账；新 Delete 不被旧失败覆盖；已获得成功结果不降级失败；已提交 Delete 可重放且不重复 HTTP |
| Client 边界 | Client tests：404 code 与 retryable 解耦；429/5xx 回退；loopback HTTP/远端 HTTPS 配置；等价 boot UUID、nil 和非法值 |

这些测试不证明 daemon health 接线、进程 abort 恢复或 durable persistence。

## 5. 直接组件接入证据

核心矩阵之外，第一版“接入 Adapter 和 AgentSight Client”的交付还必须有至少一条
本地组合测试：完整 Binding → 实际 AgentSight Adapter → 实际 AgentSight Client
（可替换 transport/process resolver）→ 内存 repository 最终状态。覆盖 Apply 成功
和 Delete 结果不确定后重试两条路径；网络使用 captured/mock wire，无需启动真实服务。

组合测试证明实际接口接通，不替代核心竞争/失败矩阵，也不证明远端实际生效。
AgentSight Client 自身负责测试 UUIDv5、prepare 无修改请求、固定请求、原地/替换
机制的具体选择、部分结果与成功/不存在响应解释。Adapter 自身负责 DSL 翻译。
若实现需扩展两者现有接口，仍按用户要求先确认具体变更。

## 6. 完成判定及证据记录

以下全部满足后，才可声明“首版 Reconciler 核心通过验收”：

1. 第 4 节全部 case/variant 有完整 fixture 和实际 runner，全部 PASS，无缺失、
   ignored 或以 TODO 代替实现；第 5 节实际组件组合测试通过。
2. runner 可由 crate-local `cargo test` 命令执行；提交时记录真实 crate/target 名称、
   HEAD、命令、case 数量及 pass/fail 结果，不把拟定包名或命令写成已验证。
3. 相关 crate 及直接消费者测试通过；workspace Clippy、format、lockfile 和
   `git diff --check` 通过。发生失败必须说明影响，不能用文档检查代替运行结果。
4. 验收记录附完整输出/trace 的位置及差异报告；稳定 prepared、目标登记先于请求、
   同 Binding 串行、revision/status CAS 和失败保留记录均有可执行证据。
5. 报告明确写明：memory-only、跨重启不恢复、mock wire、未验证真实 PEP/kernel；
   PAP API/交互块及所有延期项单独标记，不能以核心通过宣称全链路完成。

当前交付清单：

| 产物 | 当前状态 |
|---|---|
| 本验收标准与场景判定 | 已记录 |
| 完整 JSON fixtures 及 case/variant 清单 | 已实现；45 个串行变体（69 步）+ 2 个线程竞争变体 |
| crate-local runner 与 Reconciler | `asc-pcp`；核心矩阵 PASS |
| Adapter/Client 实际组合测试 | PASS；真实 Adapter/Client/Ureq + 内存 repository + loopback HTTP mock，4 tests |
| 运行结果及可执行验收报告 | [RESULTS.md](RESULTS.md)；核心及本地组件组合门禁通过，含 PAP 内存生命周期组合，不含 daemon worker/真实 PEP |

本次标准整理不要求为延期功能添加实现或测试；新增需求需明确归属及阶段，不能
通过扩大验收矩阵隐式增加首阶段范围。

## 7. 删除生命周期修正（2026-09-08）

- `pap_lifecycle.rs`：真实 PAP + memory + core，4 tests 覆盖同 spec 失败重试复用
  prepared、Delete 失败重试/预算/不可撤销、硬删除后的 NotFound/LIST、新 ID 创建、
  spec 更新保留旧清理目标，以及 revision 上限。
- `repository_contract.rs`：6 tests，包括删除的完整快照 CAS、缺失记录删除重放、
  旧 aggregate/PAP Update 无法重建 revision 1 记录。
- `src/panic_recovery_tests.rs`：9 tests，包括整体删除提交后的 panic/响应故障；只重试记账，
  不重复 Client delete，确认完成后回收槽位。
- 删除完成的 JSON expected 为 `null`。移除 REC-CORE-019/deleted（不再有此 current
  record），缺失通知由 missing 覆盖；REC-CORE-021 改为 fresh-binding。共 45 个串行
  变体、69 步，加 2 个并发变体。原 fixture 精简规则和完整 trace 校验保持不变。

PAP 请求 wire 由 daemon 的完整 CRUD fixture 及 UDS 测试验证；这些测试不会启动
reconcile worker。SQL 物理表、跨进程 CAS、重启恢复与 live AgentSight 仍无验收声明。
