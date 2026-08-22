# Phase 1 Capability Broker 验收报告

[English](acceptance.md) | [设计](design_zh.md)

## 结果

**整体结果为 PARTIAL。通用 Broker foundation 已存在，但本次没有交付 checkpoint execution target 或
ledger-side reconciliation，Phase 1 尚未通过。**
实现 worktree 基于 `a43ab81738d3f39721a425cef717a6147276fae9`。

基础 Broker logic 会在 policy 前校验 Capability request expiry，以及 authoritative Task、Run、完整 Actor
provenance、target、operation descriptor、完整 operation digest 与 requested scope。它将 policy
decision 与 permit 分开，为 permit 绑定准确 authority，并通过 process-local memory store atomically
consume single-use permit。定向 capability test 通过。这些是通用 contract/logic foundation，不是
production execution 证据。

该结果不构成通用治理声明。Production `CoshBrokered` profile 绑定固定 `task-only-v1` manifest，只有
`ask_user_question`。Daemon、durable start intent、installed Core factory 与 private v3 handshake 会在
execution 或 Task input 前校验 identity，且该 profile 不依赖 checkpoint/ws-ckpt。通用
Capability、Permit 与 Execution contract/ledger row 作为后续基础保留。Shell、Skill、MCP、扩展工具、
legacy CLI 与 interactive Core mutation path 仍在该边界之外。

## 可选 profile 与被推迟的 checkpoint target 结果

**整体结果为：封闭 profile 与 sealed provider set 已验证；checkpoint provider authority 被 withheld；
checkpoint execution target 被推迟。** Gateway 拥有封闭的 `workspace-checkpoint-v1` profile（含固定 canonical
manifest 与 digest）、sealed 单 provider 集合，以及会拒绝任何被请求的 checkpoint provider 的
`SealedCapabilityProviderRegistry`。`CkptClient` 另外获得了 effect 分类、只读 evidence 查询与 peer 认证。

**本增量中不存在 checkpoint execution target。** 一个 `workspace_checkpoint_create` permit 必须只授权创建一个
快照，而 ws-ckpt 的 checkpoint 请求无法被约束到该范围：dispatch 会无条件先跑 workspace auto-init，且在任何前置
查询与请求之间注册消失的 workspace identity 会被当作相对路径解析。Auto-init 会注册 workspace、可能收养
subvolume、把目录改名移开、创建 symlink，并删除损坏的 symlink。checkpoint-create permit 不授予其中任何一项；
Gateway 无法阻止（查询与请求是两次独立往返），也无法撤销。因此该 target 被推迟，而不是加个门之后照样交付——这也让
`cosh-gateway` 在内部 crate 中继续只依赖无副作用的 `cosh-gateway-contracts` 叶子。

为被推迟的 slice 记录了两个 ws-ckpt 协议前置条件：严格解析 workspace identity 且绝不 auto-init 的 checkpoint
请求，以及与 checkpoint 创建原子校验的不可复用 workspace generation token。设计文档另外记录了原型阶段确立的
socket 可信性、workspace 双表示、btrfs volume identity 与 generation 约束，避免重新踩坑。

`serve`、打包的 systemd unit、installed Core factory、private Core v3 mirror 与 brokered execution driver 均未
改动，因此没有 Runtime 能请求或获得 checkpoint。checkpoint-enabled instance 不能端到端启动，任何 release 都不得
宣传 governed checkpoint。

Owner scope decision 如下：下表 Phase 1 criterion 只适用于 enabled Gateway production profile。
Production `serve` 与 library daemon 只接纳 `core` / `gateway-brokered-v1`。ACP `doctor`/`run`
interoperability、standalone Shell 与 legacy CLI 明确 ungoverned；这些 path 的 compatibility evidence
不能满足 Broker criterion。Formal universal Broker exit 仍未满足。

## Durable ledger storage 结果

**整体结果为 VERIFIED DURABLE TASK STORAGE SLICE；Production Broker integration 为 NOT IMPLEMENTED。**
Checksummed SQLite schema v9 现在会持久化 Task/Run event 与 projection、approval/input state、Outbox intent、
Runtime binding、fenced Run lease 与 durable dispatch receipt。通用 permit/execution ledger contract 作为
后续基础保留；本 PR 不接入 production target，也不声称 checkpoint execution、typed side-effect result
或 reconciliation。

Durable ledger suite 使用
`cargo test --locked --package cosh-gateway storage --no-fail-fast` 复现。其 fixture 覆盖 stale lease、
cross-Task Run、generation skip、扩大
approval deadline、integer overflow、idempotency namespace、receipt corruption 与 atomic rollback。

本 PR 没有把 checkpoint driver 或 pre-effect execution loop 接入 durable ledger。Durable permit
issuance、audit gate 与 Runtime 可见的 checkpoint execution 属后续工作。Trusted config 不会接纳任何
checkpoint execution target，Task 也无法触达这类 target。

## 结果口径

| 结果 | 含义 |
| --- | --- |
| PASS | 可复现证据满足所述 scope 的完整验收项。 |
| PARTIAL | 实现证据只满足明确列出的子集。 |
| FAIL | 当前启用 path 违反目标 invariant。 |
| NOT IMPLEMENTED | 该验收项没有实现。 |
| BLOCKED | 前置决策阻止验证。 |

## 实现证据

| 来源 | 已验证行为 |
| --- | --- |
| [`capability.rs`](../../../../../crates/cosh-gateway/src/capability.rs) | 公开 Broker、policy、permit-store、claim、context 与 memory-store 边界，不公开 executor |
| [`broker.rs`](../../../../../crates/cosh-gateway/src/capability/broker.rs) | 在 policy 前校验 expiry、Task/Run/完整 `ActorRef` 与 authoritative target/descriptor/完整 operation digest/scope；拒绝 unavailable 或 invalid policy authority，并公开 atomic claim |
| [`memory.rs`](../../../../../crates/cosh-gateway/src/capability/memory.rs) | 在同一个 mutex 内校验并 consume permit；mismatch、expiry 与 replay fail closed |
| [`memory/tests.rs`](../../../../../crates/cosh-gateway/src/capability/memory/tests.rs) | 覆盖 parent 与 actor-provenance substitution、policy branch/failure、permit binding、mismatch、expiry/replay 与 concurrent consumption |
| [`capability.rs`](../../../../../crates/cosh-gateway-contracts/src/capability.rs) | 定义中立 request、decision、approval 与 permit contract，包含 Actor/Task/Run/Execution/target/operation/policy/expiry binding |
| [`profile.rs`](../../../../../crates/cosh-gateway-contracts/src/profile.rs) | 固定 `task-only-v1` 与 `workspace-checkpoint-v1` identity、canonical manifest digest、governed target、准确 Runtime inventory 与 sealed provider set |
| [`provider.rs`](../../../../../crates/cosh-gateway/src/capability/provider.rs) | 只接纳 profile 固化的 provider 集合；拒绝 task-only instance 上被请求的 provider 与 checkpoint-enabled instance 上缺失的 provider，并对所有 profile withhold checkpoint provider |
| [`checkpoint.rs`](../../../../../crates/cosh-platform/src/checkpoint.rs) | 在任何写入之前认证已连接的 peer，根据请求阶段返回可证明无副作用或 `PossiblyApplied`，并提供准确的只读 evidence 查询。目前还没有 Gateway 调用方；这些是未来 target 与 ledger-side reconciler 所需的 transport 原语 |
| [`scheduler.rs`](../../../../../crates/cosh-gateway/src/daemon/scheduler.rs) | 保持 durable Task/Run/Outbox/lease/input/cancel/retry/recovery coordination 与未来 execution-target adapter 分离 |

Broker source 只依赖 contracts 和两个显式 port，不 import Task storage、Runtime bridge、OS operator、
ACP 或 network API。

## 验收矩阵

| ID | 验收项 | 结果 | 证据或剩余缺口 |
| --- | --- | --- | --- |
| CBR-001 | Gateway production profile 启用的每个 side effect 使用 typed `CapabilityRequest`。 | Task-only scope PASS | Immutable inventory 只有无 side effect 的 `ask_user_question`；production 没有 enabled side effect。 |
| CBR-002 | 其 target 解析成 immutable authenticated local identity。 | NOT IMPLEMENTED | 不存在 checkpoint execution target。原型阶段确立的 identity 要求——socket 路径链加 peer 认证、workspace 的两种表示、`(filesystem ID, subvolume ID, inode)` 形式的 btrfs volume identity——已记录在设计文档中，作为被推迟 slice 的前置条件。 |
| CBR-003 | 该 path 的 policy result、approval 与 permit 是不同类型。 | PARTIAL | 通用 contract 与 in-memory logic 区分这些类型；production 没有 side-effect path 签发它们。 |
| CBR-004 | 该 profile 每个 permitted effect 有一个 `ExecutionId`。 | NOT IMPLEMENTED | Task-only inventory 没有 permitted OS effect 或 production execution target。 |
| CBR-005 | 其 permit 绑定 actor、Task、Run、target、operation digest、policy、fence、expiry 与一次使用。 | PARTIAL | 通用 permit validation 在 logic test 中覆盖 binding；durable production issuance 属后续工作。 |
| CBR-006 | 其 target 在执行前立即校验并 consume permit。 | NOT IMPLEMENTED | 不存在 production target，因此不声称 target-side consume 或 execution loop。 |
| CBR-007 | 其 approval 是 durable Task state 且不能扩大 authority。 | PARTIAL | Durable Task approval state 已有；approval-to-permit 与 target binding 属后续工作。 |
| CBR-008 | Broker 不写 Task aggregate。 | PASS | Broker 不依赖 Task aggregate 或 storage，只返回 decision。 |
| CBR-009 | 该 profile 重复 execute 不能产生第二个 effect。 | PARTIAL | 没有已准入 provider 会启用 effect。Transport 提供只读 evidence 原语，未来 target 可在响应丢失后用它代替再次 create，但当前不存在 execution target 或 permit loop。 |
| CBR-010 | Crash uncertainty 进入 typed reconciliation，不自动 retry。 | PARTIAL | Task/Run restart recovery 与 fail-closed retry boundary 已有。第一个请求字节之后发生任何失败时，checkpoint transport 都返回 `PossiblyApplied`，包括 daemon 报告错误码的情况，并公开只读 evidence 原语。Target 侧与 ledger 侧 reconcile 属后续工作。 |
| CBR-011 | Governed profile 不启用 opaque Shell fallback。 | Task-only scope PASS | Accepted inventory 不含 Shell 或其他 side-effect operation；legacy parser behavior 只算 compatibility evidence。 |
| CBR-012 | Typed policy 有 allow/deny/require-approval outcome。 | PASS | Neutral `PolicyPort` 与 deterministic test 覆盖三个 outcome，以及 unavailable/invalid authority。 |
| CBR-013 | 该 profile 的 execution start 要求 durable security audit。 | NOT IMPLEMENTED | Task-only profile 没有 production execution start 或 target。 |
| CBR-014 | 该 profile 禁用或 delegated Core direct side-effecting tool。 | Task-only scope PASS | Immutable inventory 只有 `ask_user_question`；hook、MCP、Skill、扩展、Shell、file、process、network 与 checkpoint path 均禁用。 |
| CBR-015 | Governed mode 下 production Gateway operation 不能绕过 permit。 | Task-only scope PASS | `serve` 与 library daemon 从固定 profile 派生 target；Runtime start schema v3 与 Core handshake 会在 launch/input 前拒绝 identity 或 inventory drift。没有 production side-effect operation 可以绕过 permit。ACP `doctor`/`run` 与 legacy CLI 明确不在 governed claim 内。 |
| CBR-016 | Remote identity 在 v2 attestation 决策批准前保持禁用。 | Scope decision PASS | Phase 1 是 local installation-scoped single-tenant；remote 与 `TenantId`/multi-tenant support 属未来 v2。 |

## 验证证据

从 `src/cosh-ng` 在 rebased candidate 上运行：

```text
cargo fmt --all -- --check
cargo test --locked -p cosh-gateway-contracts profile::
cargo test --locked -p cosh-platform checkpoint::
cargo test --locked -p cosh-gateway
cargo clippy --locked -p cosh-gateway-contracts -p cosh-platform -p cosh-gateway --all-targets -- -D warnings
cargo doc --locked --no-deps -p cosh-gateway-contracts -p cosh-gateway
```

定向测试证明：

- Request expiry，以及 Task、Run、Actor ID、issuer、assurance、target、operation descriptor、
  完整 operation digest 与 scope substitution 在 policy 前 fail closed；
- Policy deny 与 approval 不会创建 permit；
- Policy unavailable、revision 为零与 authority 过期 fail closed；
- Issued permit 绑定 actor、Task、Run、Execution、target、完整 canonical operation digest、policy revision、expiry
  与一次使用；
- 错误 actor、Task、Run、Execution、target、完整 operation digest 或 policy revision 不 consume authority；
- Expired 与重复 consume fail closed；
- 八个同时 claim 只有一个成功。

可选 profile 与被 withhold 的 provider 测试另外证明：

- `task-only-v1` canonical manifest 与固定 digest 未变且不含 provider 段，因此 private Core v3 mirror
  仍校验同一个 identity；
- 第二个 profile 的 manifest、digest、准确两项 tool inventory 与单 provider 集合会拒绝任何缺失、额外、
  重排、改名或替换的取值，含已被拒绝的 `ws-ckpt-v1` 名称；
- Task-only instance 在没有 ws-ckpt socket、目录与 daemon 时接纳空 provider 集合，并拒绝已配置的
  checkpoint provider 而不是扩权；
- Checkpoint-enabled instance 在没有请求 provider 时拒绝准入；
- 固化该 provider 的 profile 同样被 withhold，且穷举 (profile × requested provider) 全部组合的 test 表明没有任何
  准入结果会产生 checkpoint 副作用 authority；
- 每个 transport 阶段都返回可证明无副作用或 `PossiblyApplied`，覆盖全部十三个 daemon 错误码，并覆盖 peer
  认证与准确的只读 evidence 查询。

通用 contract/ledger 与 scheduler suite 已执行。Destructive package-unit containment fixture 已在
disposable Ubuntu 24.04 arm64/systemd 255 上通过。Checkpoint 证据只来自 fake Unix daemon；不声称已验收
真实 ws-ckpt daemon、真实 Codex/Claude、人工 Terminal、ECS、network 或通用工具验证。

本增量没有验证任何 btrfs 行为，因为没有任何代码读取它。btrfs volume identity 要求被记录在设计文档中而非实现，
确立该要求的原型保存在交付分支之外。验证它需要特权 btrfs 环境，属于被推迟的 slice。

## 必要的剩余产物

| 产物 | 必须提供的证明 |
| --- | --- |
| 通用 approval 与 re-authorization test | 每个 presentation path 都只能从 committed matching approval 签发 authority。 |
| Immutable target-substitution matrix | Workspace、UID、boot、container 与 instance change 使 permit 失效。 |
| Multi-operation permit/execution matrix | Generic invariant 对每个未来 governed operation 都成立。 |
| 通用 security audit gate | 每个 target 的 required audit persistence 失败时 issuance 与 execution start 都失败。 |
| Execution kill-point 与 reconciliation matrix | Claimed、started 与 uncertain effect 绝不自动 replay。 |
| Broker bypass inventory | 每个 enabled Gateway/Core/Shell/ACP/Skill/MCP effect 都到达 verifier。 |
| Revocation 与 lease-fence corpus | Revoked、stale-runtime 与 stale-policy authority fail closed。 |
| Trusted canonicalizer test | Independent canonicalization 在 Broker admission 前绑定 descriptor 与 digest。 |

## Exit Criteria

Task-only production profile 只满足有界 inventory 决策，Phase 1 仍为 PARTIAL，因为 production target
execution 尚未实现，已记录的 real-provider/manual release gate 也仍开放。Formal universal Broker exit 还要求：

1. 每个未来 enabled operation 的 approval resolution、permit issuance、durable consumption、audit、
   execution 与 typed reconciliation 形成一个经过评审的 security boundary。
2. 通用 immutable target identity、revocation 与 remote attestation 决策。
3. Bypass inventory 覆盖每个未来 enabled Gateway/Core/Shell/ACP/Skill/MCP mutation edge。
4. Crash、replay、substitution、audit-failure、revocation、real-provider 与 manual fixture 在同一个准确
   release candidate 上通过。

## 剩余风险

- `MemoryPermitStore` 仍只用于测试；任何 future target path 的 production authority 尚未接入。
- 当前没有实现 checkpoint execution target，Runtime 也无法触达这类 target。不得把 checkpoint profile 与
  withheld provider 解读为 governed checkpoint 支持。
- Transport 只提供按 workspace 查询的只读 evidence 原语。如果该证据变得有歧义或无界，未来 ledger-side
  reconciler 必须先获得窄的 exact-query 协议，才能记录确定结论。
- 任何 ws-ckpt 响应码都不被当作 pre-effect 证据，因此普通的 daemon 拒绝会让 transport 返回
  `PossiblyApplied`。当前不会记录 durable uncertain receipt；未来 target 与 ledger 必须持久化这种不确定性，
  直到 reconciliation 完成。要减少这种不确定性，需要协议提供明确的 pre-effect 保证，而不是维护一张对着
  daemon 内部实现的分类表。
- Daemon 仍是其自身 workspace registry 的信任锚，被攻陷的 daemon 在该边界之外。
- **Checkpoint 请求不是 identity-only。** ws-ckpt 在每次 checkpoint 之前都会跑 workspace auto-init，并把未注册的
  workspace identity 当相对路径解析，因此 checkpoint-create permit 可能在其 authority 之外导致 workspace 注册或
  symlink 删除。Provider 准入将一直 withheld、execution target 一直推迟，直到协议提供严格 identity-only 的请求。
  这是 authority 缺口而非上报缺口，Gateway 侧没有任何检查能关闭它。
- **Gateway 侧 checkpoint transport 还没有合法归属。** 在内部 crate 中 `cosh-gateway` 只依赖
  `cosh-gateway-contracts`。从 Gateway target 复用 `CkptClient` 会新增 `cosh-platform` 与 `cosh-types` 两条边，
  因此被推迟的 slice 必须先决定该 transport 应该放在哪里。
- **Generation 归因未被 fence。** ws-ckpt workspace identity 由 workspace 路径派生，unregister 之后可被复用；
  `rollback` 到当前 DAG head 会替换 live subvolume，同时保持 workspace ID、注册路径与 `index.head` 不变。未来
  target 必须在每次请求前后都把注册路径、workspace volume identity 与 daemon 映射同准入状态比较。原型表明，
  使用 btrfs 文件系统标识加不被复用的 subvolume ID，可以检出持续到任一次比较时的 rollback；完全落在 create
  窗口内的 rollback 仍不可见，因为没有协议取值可被原子校验。因此在 ws-ckpt 协议提供与 checkpoint 创建原子
  校验的不可复用 workspace generation token 之前，未来 ledger 不得把确定性 receipt 绑定到 workspace 内容的
  generation，也不得声称存在 immutable workspace fence。
- Ungoverned Shell、ACP interoperability、Skill、MCP、扩展与 legacy effect 仍在封闭 production
  profile 之外，绝不能宣传为 governed。
