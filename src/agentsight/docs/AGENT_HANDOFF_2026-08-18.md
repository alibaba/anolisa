# AgentSight 开发交接说明（2026-08-18）

> 面向接管 AgentSight 开发的新 Agent。先读本文件，再读 `src/agentsight/AGENTS.md`、`docs/ARCHITECTURE.md`、`docs/DEVELOPMENT.md` 和 `docs/PITFALLS.md`。本文区分“已提交”“未提交”“只有设计”“需要真实 Linux 验证”，不要把 Dashboard 状态当成内核生效证明。

## 1. 项目要解决什么

AgentSight 原本是基于 eBPF 的 Agent 可观测与审计组件，现在已经向系统级安全闭环扩展：把 Agent、会话、进程树、文件访问、敏感状态传播、网络连接、策略判定和内核结果关联起来，并允许操作者把已确认的风险从审计升级为临时处置。

目前最重要的用户故事有两个：

1. **一键防护**：在 Agent 看板上识别某个运行中的 Agent，扫描其工作目录中的凭据类文件，为该进程树建立凭据外泄审计策略。默认只记录、不阻断。
2. **审计到拦截**：系统审计把连续行为聚合成风险案件；操作者确认后，尝试将原审计策略临时升级为内核拦截，到期或解除后恢复原审计策略。

安全口径必须严谨：敏感文件被读取后访问陌生公网，只能说明形成了值得调查的连续行为，不能直接宣称凭据已经泄漏。只有真实执行点确认策略挂载，并且内核返回真实拒绝结果，页面才能显示“已拦截”。

## 2. 代码位置和分支状态

当前工作目录 `/Users/xzw/Program/anolisa` 本身不是 Git worktree。真正包含一键防护代码的 worktree 是：

```text
/Users/xzw/Program/anolisa/.worktrees/deploy-121-199-33-125
```

分支与提交：

```text
branch: deploy/121-199-33-125
e3d31d72 feat(sight): add one-click agent audit protection
6bfc801b fix(sight): hide enforcement backend branding
```

截至 2026-08-18，该分支相对 `origin/main`：

```text
ahead 2, behind 12
```

远端分支中没有发现包含 `e3d31d72` 的分支，不能假定一键防护已经合入主干。

当前 worktree 不是干净状态：

```text
M src/agentsight/src/server/enforcement.rs
```

这份未提交修改主要做了三件事：

- 再次打开“一键防护”设置时，从当前有效策略快照恢复敏感文件和可信目标，而不是重新扫描后丢掉原配置；
- 禁止把文件系统根目录 `/` 作为扫描目录；
- 增加策略恢复相关单元测试。

不要执行 `git reset --hard`、`git checkout --` 或直接删除 worktree。先保存这份 diff、格式化并完成测试，再决定是补提交还是重做。

## 3. 已经完成的能力

### 3.1 系统审计

代码已经具备独立的系统审计领域和 Dashboard：

- 审计事件持久化与查询；
- 风险案件聚合；
- Agent、会话、PID、进程启动时间和策略身份关联；
- 文件、taint、网络、策略判定、执行状态等证据展示；
- 风险案件保持原始证据不可变，处置生命周期追加在同一案件上。

关键位置：

```text
src/agentsight/crates/agentsight-audit/
src/agentsight/src/security/
src/agentsight/dashboard/src/pages/SystemAuditPage.tsx
```

### 3.2 风险执行与处置基础设施

已存在：

- 独立的 `agentsight-enforcer` 特权进程；
- AgentSight 与 enforcer 之间的本地协议；
- binding、策略快照、违规事件和状态持久化；
- apply、detach、replace、恢复、幂等和重试相关代码；
- Dashboard 的“风险观察与审计/风险拦截”页面；
- 案件确认、临时处置、到期恢复的生命周期模型；
- 产品页面隐藏 ActPlane 等后端品牌和原始 DSL。

关键位置：

```text
src/agentsight/src/enforcement/
src/agentsight/crates/enforcement-protocol/
src/agentsight/crates/agentsight-enforcer/
src/agentsight/src/security/containment/
src/agentsight/dashboard/src/pages/RiskEnforcementPage.tsx
src/agentsight/dashboard/src/components/ContainmentLifecycleCard.tsx
```

### 3.3 一键防护

提交 `e3d31d72` 在 Agent 卡片增加了“一键开启”和“设置”入口。当前流程是：

```text
读取 /proc/<pid>/cwd
→ 扫描工作目录中的敏感文件名
→ 用户确认文件和可选可信目标
→ POST /api/enforcement/credential-bindings
→ 创建 Audit 模式的 CredentialExfiltrationPolicy
→ enforcer 挂载 notify 策略
→ Dashboard 显示审计保护状态
```

扫描规则：

- 最大深度 3，最多 64 个文件；
- 识别 `.env`、`.env.*`、包含 `credential` 的文件、`.npmrc`、`.pypirc`、`id_rsa*`、`id_ed25519*`；
- 跳过 `.git`、`node_modules`、`target`、`dist`、`build`、`.cache`；
- 不读取文件正文；
- 当前策略要求文件已经存在并能 canonicalize；
- 当前运行时最多支持一个可信网络目标；
- taint TTL 默认 900 秒；目标范围固定为 `public_ipv4`；
- Agent 重启后 PID/启动时间身份变化，需要重新绑定。

新增接口：

```text
GET  /api/enforcement/agent-protection/{pid}
POST /api/enforcement/credential-bindings
GET  /api/enforcement/bindings
```

## 4. 最容易误解的地方

### 4.1 “一键防护”目前是审计，不是网络阻断

前端明确提交：

```text
mode: audit
destination_scope: public_ipv4
```

当前固定版本的 ActPlane ABI 可以生成 `notify connect` 规则，并由 Adapter 在事件侧做 TTL、可信目标和公网分类过滤；但不能在 LSM 决策前同时准确满足 TTL 与 `public_ipv4` 语义。因此真实后端会拒绝 `PolicyMode::Enforce`：

```text
the pinned ActPlane ABI cannot enforce taint TTL and public_ipv4 destinations
without weakening product semantics
```

这是一项正确的 fail-closed 设计。不要为了演示把 Audit 偷换成 Enforce，也不要把“binding state = enforced”翻译成“网络已经被内核阻断”。这里的 `enforced` 只说明该 binding 已被执行端接受，实际效果仍由 `policy_mode` 和违规事件的 `effect/blocked` 决定。

### 4.2 文件打开阻断与凭据外泄阻断不是同一个能力

真实后端已经支持简单的敏感文件打开阻断：对一个已存在的绝对普通文件，为指定 PID/进程树生成 `block open file` 规则。这不等于“读取后再阻止不可信公网”的完整策略。

### 4.3 审计到拦截的无空窗切换尚未在真实 ActPlane 上成立

设计和存储层已经实现 leased compare-and-replace、持久化 intent、回滚和恢复模型。但固定 ActPlane ABI 不能准备第二个权威 profile，也不能在内核/map 边界原子切换。因此真实 Adapter 会保留原审计策略并返回 `UnsupportedHandoff/SourceRetained`，不会假装升级成功。

Mock 后端可以演示完整生命周期，不能据此宣称生产后端支持。

详见：

```text
src/agentsight/docs/design/audit-enforcement-handoff.md
src/agentsight/crates/agentsight-enforcer/src/actplane.rs
```

## 5. 尚未完成或需要优先修复

### P0：接管后第一批工作

1. **处理未提交修改**：先运行 rustfmt，检查恢复当前策略配置的逻辑，补提交；不要覆盖用户改动。
2. **同步主干**：分支落后 12 个提交。先评估 rebase/merge 冲突；其中 AgentSight 相关的新提交至少包括 raw packaging 和 latency 修复。
3. **补一键防护的前端回归测试**：当前提交修改了 `AgentHealthPage.tsx`，但已有前端回归测试没有覆盖按钮、空扫描、设置恢复、API 失败和重复提交。
4. **定义重复开启/修改策略的语义**：每次提交都会生成新 binding UUID。已有 active binding 时再次点击“开启审计保护”可能发生 singleton conflict。应明确为 Replace、先安全解除再建、或拒绝并提示，不能静默创建第二条。
5. **增加关闭入口**：Agent 卡片只有“设置”，缺少直接解除保护；目前只能去风险执行页面解除。
6. **在 Linux 上跑完整门禁**：macOS 不能覆盖 server/enforcement 和 BPF 路径。

### P1：真实环境验收

1. 验证 OS、架构、内核、BTF、BPF LSM、cgroup 和 ActPlane 版本；
2. 启动 `agentsight-enforcer` 后再启动 AgentSight；
3. 使用非生产 mock credential 做完整审计链路；
4. 验证文件读取、taint 继承、子进程网络连接和案件证据；
5. 验证 Audit 不阻断；
6. 验证简单文件打开 Enforce 能得到真实 `EPERM/-EACCES`；
7. 只有 `blocked=true` 且内核结果真实拒绝时，页面才显示已阻断；
8. 验证解除后恢复访问、重启后状态重建、执行器失联时页面降级；
9. 不要在没有 ABI 支持的情况下验收凭据外泄 Enforce 或无空窗 handoff。

部署计划位于：

```text
/Users/xzw/Program/anolisa/docs/superpowers/plans/
2026-08-17-agentsight-new-instance-deployment.md
```

该计划当前仍为未勾选状态，仓库中没有足够证据证明新实例验收已经完成。服务器地址和 Dashboard Token 应向项目负责人重新确认，不要从聊天记录、shell history 或旧配置里复用密钥。

### P2：与 AgentSecCore 的后续联动

边界方向已经基本确认：

- AgentSecCore 管 Source/Resolved Policy、审批、版本和 Desired State；
- AgentSight 管执行能力检查、翻译为 ActPlane 制品、部署、Run Binding、Current State 和原始回执；
- ActPlane 负责真实同步执行。

建议正式 API 使用统一的 Policy 抽象和事务语义：

```text
GetCapabilities
PreparePolicy
CommitPolicy
AbortPolicy
GetPolicyStatus
RevokePolicy
```

`CompileResolvedPolicy` 如果保留，只作为 Dry Run/诊断接口；否则作为 `PreparePolicy` 内部步骤。不要让 AgentSecCore 生成、携带或解析 ActPlane DSL。

## 6. 开发与验证命令

进入正确 worktree：

```bash
cd /Users/xzw/Program/anolisa/.worktrees/deploy-121-199-33-125/src/agentsight
```

前端：

```bash
cd dashboard
npm run typecheck
npm run test:api-client
npm run test:i18n
npm run test:product-branding
npm run build:embed
```

Rust：

```bash
cargo fmt --all -- --check
python3 scripts/check-arch-boundaries.py
cargo clippy --all-targets -- -D warnings
cargo test
```

真实执行链路只能在 Linux 上验证。最低文档要求存在差异：顶层 BUILDING 写 Linux kernel >= 5.10，组件旧文档部分写 >= 5.8。接管后应以真实使用的 BPF-LSM/ActPlane 能力为准，统一成一份能力探测结果，不要只检查版本号。还要确认：

```text
/sys/kernel/btf/vmlinux
BPF LSM 是否启用
cgroup 能力与 namespace 身份
clang/llvm >= 15
libbpf
ActPlane 固定 revision a62e5d9d96f91101cda019519053e950d532380a
```

## 7. 已验证状态（2026-08-18）

本次交接检查实际执行结果：

- Dashboard TypeScript typecheck：通过；
- API client 回归测试：10/10 通过；
- i18n 回归测试：4/4 通过；
- 产品品牌回归测试：2/2 通过；
- Rust `cargo fmt --check`：未通过，原因是 `src/server/enforcement.rs` 的未提交代码尚未格式化；
- Rust 完整测试：macOS 环境因 Linux-only 模块的既有集成测试导入失败，不能替代 Linux CI；
- 真实 BPF/ActPlane E2E：本次未执行。

## 8. 开发经验和避坑指南

1. **状态名不等于效果证明**：`binding=enforced`、HTTP 200 或 Tool 返回成功，都不能证明内核拒绝；看真实回执和 `blocked=true`。
2. **PID 不足以标识执行主体**：至少绑定 PID + process start time；Run 场景还需 execution domain/cgroup/identity epoch。
3. **不要暴露后端实现**：Dashboard 保持 AgentSight 品牌，不展示 ActPlane DSL、后端错误、临时句柄和敏感文件内容。
4. **不要读取凭据正文**：扫描只基于路径和文件名；审计链只证明接触过敏感资源以及后续行为关联，不证明内容已经发送。
5. **能力不足要显式降级**：不支持应返回 PARTIAL/UNSUPPORTED，不能用宽松实现冒充 EXACT。
6. **超时后先查状态**：策略变更不能盲目重试，避免重复 binding 或未知状态被覆盖。
7. **配置文件是 replace，不是 merge**：`/etc/agentsight/config.json` 会完全替换内嵌 Agent 发现规则，漏掉规则会导致 Agent 消失。
8. **前端嵌入需要重新构建**：修改 Dashboard 后必须 `npm run build:embed`，再构建 Rust；否则服务器仍提供旧前端。
9. **不要在 macOS 宣称 eBPF 验收通过**：macOS 只适合部分编译、前端和纯逻辑检查。
10. **真实后端与 Mock 必须分开表述**：Mock 生命周期成功不代表固定 ActPlane ABI 支持生产拦截。
11. **新功能先扩展现有模块**：遵守 `AGENTS.md` Footprint Ladder；PR diff 尽量不超过 800 行。
12. **所有策略变更保留可恢复证据**：原审计策略、策略快照、transition intent、回执和失败阶段不能被新结论覆盖。

## 9. 建议新 Agent 的上手顺序

1. 阅读本文件和 AgentSight 四份核心开发文档；
2. 进入正确 worktree，查看 `git status`、两条独有提交和未提交 diff；
3. 先格式化并补测一键防护，不立即做大规模重构；
4. 解决重复开启、修改和解除的产品语义；
5. 同步 `origin/main` 并跑前端门禁；
6. 在支持 BPF-LSM 的 Linux 机器跑 Audit E2E 和简单文件阻断 E2E；
7. 把验收证据和环境能力矩阵写回文档；
8. 最后再推进 AgentSecCore 的 Prepare/Commit Policy API，不要把控制面接口改造和一键防护修复混在同一个 PR。

## 10. 完成定义

“一键防护完成”至少应同时满足：

- 用户能从 Agent 卡片看到、开启、修改和解除审计保护；
- 重复操作幂等，刷新后能恢复真实配置；
- Agent/PID 变化不会把旧策略错误绑定到新进程；
- 扫描不读取正文、不允许根目录、不越过明确范围；
- 执行器不可用时 fail closed 或明确降级，不显示虚假成功；
- Dashboard、API、SQLite 和 enforcer 对同一 binding 的状态一致；
- Audit E2E 在真实 Linux 通过；
- 若声称“拦截”，必须有实际内核拒绝和完整 Effect Receipt；
- 分支已同步主干、工作区干净、门禁通过并形成可 review 的 PR。
