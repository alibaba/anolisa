# Phase 1 Capability Broker 验收报告

[English](acceptance.md) | [设计](design_zh.md)

## 结果

**整体结果为 PARTIAL。Process-local Broker 逻辑切片通过，Phase 1 尚未通过。**
实现 worktree 基于 `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`。

Gateway 现在会在 policy 前校验 Capability request expiry，以及 authoritative Task、Run、完整 Actor
provenance、target、operation descriptor、完整 operation digest 与 requested scope。它将 policy
decision 与 permit 分开，为 permit 绑定准确 authority，并通过 process-local memory store atomically
consume single-use permit。八个 targeted test 通过。

该结果不构成端到端治理声明。`MemoryPermitStore` 不持久。Approval resolution 与 re-authorization
不存在。Immutable target resolver、lease/runtime fence、durable permit/execution ledger、audit gate、
OS executor、revocation、crash recovery、reconciliation、network API 与 ACP integration 均未实现。
现有 CLI/Core/Shell mutation path 仍会绕过该切片。

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

Broker source 只依赖 contracts 和两个显式 port，不 import Task storage、Runtime bridge、OS operator、
ACP 或 network API。

## 验收矩阵

| ID | 验收项 | 结果 | 证据或剩余缺口 |
| --- | --- | --- | --- |
| CBR-001 | 所有 side-effect request 使用 typed `CapabilityRequest`。 | FAIL | Broker input 已类型化，但现有 CLI/Core/Shell mutation path 绕过它。 |
| CBR-002 | Target 解析成 immutable authenticated identity。 | NOT IMPLEMENTED | 第一版只绑定 exact `TargetRef`；没有 resolver、boot/workspace/UID identity 或 attestation。 |
| CBR-003 | Policy result、approval 与 permit 是不同类型。 | PARTIAL | `PolicyDecision`、`ApprovalRequest`、`CapabilityDecision` 与 `ExecutionPermit` 已分离；durable approval resolution/re-authorization 不存在。 |
| CBR-004 | 每个 permitted effect 有一个 `ExecutionId`。 | PARTIAL | 每个 Broker-issued permit 都有一个 `ExecutionId`；bypass path 与 execution lifecycle 未集成。 |
| CBR-005 | Permit 绑定 actor、Task、Run、target、operation digest、policy、fence、expiry 与一次使用。 | PARTIAL | Actor/Task/Run/Execution/exact target/完整 operation digest/policy/expiry/single-use binding 通过；immutable target 与 runtime/lease fence 尚缺。Canonicalization 与 hashing 仍依赖 trusted ingress。 |
| CBR-006 | Target 在执行前立即校验并 consume permit。 | PARTIAL | `claim` atomically 校验并 consume，但没有 target adapter 或 durable store 在 effect 前调用。 |
| CBR-007 | Approval 是 durable Task state 且不能扩大 authority。 | PARTIAL | Approval request 保留 request/Task/Run/expiry；没有 durable approval ledger、resolution 或 approval-bound issuance。Direct permit 的 `approval_id = None`。 |
| CBR-008 | Broker 不写 Task aggregate。 | PASS | Broker 不依赖 Task aggregate 或 storage，只返回 decision。 |
| CBR-009 | 重复 execute 不能产生第二个 effect。 | PARTIAL | 八个 concurrent claim 只有一个成功，单进程 replay 失败；没有跨 restart 的 durable execution/effect ledger。 |
| CBR-010 | Crash uncertainty 进入 typed reconciliation，不自动 retry。 | NOT IMPLEMENTED | 没有 execution lifecycle 或 reconciliation port。 |
| CBR-011 | Shell parsing 对 adversarial separator 与 metacharacter fail closed。 | PASS | 当前 parser/heuristic baseline 仍可复用；新 Broker 没有增加 Shell fallback。 |
| CBR-012 | Typed policy 有 allow/deny/require-approval outcome。 | PASS | Neutral `PolicyPort` 与 deterministic test 覆盖三个 outcome，以及 unavailable/invalid authority。 |
| CBR-013 | Permit issuance 与 execution start 要求 durable security audit。 | NOT IMPLEMENTED | Memory store 没有 audit port 或 durable issuance gate。 |
| CBR-014 | Brokered mode 禁用或 delegated cosh-core direct side-effecting tool。 | FAIL | Brokered Core integration 不存在。 |
| CBR-015 | Governed mode 下 CLI/platform operation 不能绕过 permit。 | FAIL | Governed operator integration 不存在。 |
| CBR-016 | Canonical remote target identity/attestation 已批准。 | BLOCKED | Target identity 决策与实现仍开放。 |

## 验证证据

从 `src/cosh-ng` 运行：

```text
cargo fmt --package cosh-gateway -- --check
cargo test --locked --package cosh-gateway-contracts
result: 6 integration tests passed；unit 与 doc-test target passed

cargo test --locked --package cosh-gateway capability::
result: 8 passed；0 failed；38 filtered out

cargo clippy --locked --package cosh-gateway-contracts --all-targets -- -D warnings
cargo clippy --locked --package cosh-gateway --all-targets -- -D warnings
result: passed with zero warnings

cargo doc --locked --package cosh-gateway-contracts --package cosh-gateway --no-deps
result: passed
```

八个测试证明：

- Request expiry，以及 Task、Run、Actor ID、issuer、assurance、target、operation descriptor、
  完整 operation digest 与 scope substitution 在 policy 前 fail closed；
- Policy deny 与 approval 不会创建 permit；
- Policy unavailable、revision 为零与 authority 过期 fail closed；
- Issued permit 绑定 actor、Task、Run、Execution、target、完整 canonical operation digest、policy revision、expiry
  与一次使用；
- 错误 actor、Task、Run、Execution、target、完整 operation digest 或 policy revision 不 consume authority；
- Expired 与重复 consume fail closed；
- 八个同时 claim 只有一个成功。

该切片没有相关 adapter，因此未运行 ECS、provider、OS mutation、network 或 ACP validation。

## 必要的剩余产物

| 产物 | 必须提供的证明 |
| --- | --- |
| Durable approval 与 re-authorization test | 只有 committed、matching approval 可以签发 no-wider permit。 |
| Immutable target-substitution matrix | Workspace、UID、boot、container 与 instance change 使 permit 失效。 |
| Durable permit/execution ledger | Restart 不能恢复 consumed authority，也不能重复 known/unknown effect。 |
| Security audit gate | Required audit persistence 失败时 issuance 与 execution start 失败。 |
| Execution kill-point 与 reconciliation matrix | Claimed、started 与 uncertain effect 绝不自动 replay。 |
| Broker bypass inventory | 每个 enabled Gateway/Core/Shell/ACP/Skill/MCP effect 都到达 verifier。 |
| Revocation 与 lease-fence corpus | Revoked、stale-runtime 与 stale-policy authority fail closed。 |
| Trusted canonicalizer test | Independent canonicalization 在 Broker admission 前绑定 descriptor 与 digest。 |

## Exit Criteria

1. CBR-001 至 CBR-015 全部 PASS；CBR-016 通过前禁用 remote execution。
2. Approval resolution、permit issuance、durable consumption、audit、execution 与 reconciliation
   形成一个经过评审的 security boundary。
3. Immutable target identity 与 runtime/lease fencing 替换初始 `TargetRef` binding。
4. Bypass inventory 覆盖所有 enabled mutation edge，不存在 direct executor path。
5. Crash、replay、substitution、audit-failure 与 revocation fixture 在同一个准确 commit 上通过。

## 剩余风险

- Process restart 会清空 `MemoryPermitStore`，因此它不能保护 production authority。
- Unresolved approval 不能授权 permit；如果没有 durable matching 就增加该 path，会产生扩大 authority 的漏洞。
- Exact `TargetRef` equality 无法发现 boot、UID、namespace、symlink 或 workspace change。
- Caller 仍可通过当前 Core、CLI 与 platform execution path 绕过 Broker。
- 没有 durable execution state 的 permit claim 无法 reconcile effect 后的 crash。
