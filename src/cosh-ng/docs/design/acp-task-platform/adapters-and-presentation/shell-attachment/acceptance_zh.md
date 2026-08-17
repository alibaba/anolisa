# Phase 2 Shell Attachment 验收报告

[English](acceptance.md)

相关设计：[Shell Attachment 设计](design_zh.md)。

## 1. 报告范围

- 审计基线：`6c115aefe04ace0d169a24fa7cd55ad7c1befa52`
- 审计日期：2026-08-12
- 变更类型：仅规划文档
- 实现验收：**NOT ACCEPTED**

这是当前 readiness baseline 和未来 exit gate。本次文档变更没有实现或在线
测试任何 Task attachment 行为。

## 2. 基线证据

基线 Shell 拥有 PTY 与 foreground child，保持原生 bash/zsh 行为，转发 raw
input/output，渲染 inline card，并在 `InlineState` 保存大量 Runtime state。它
还有 provider session Adapter 与脱敏的 Shell event journal。

基线没有 Gateway Client、持久 Task attachment、projection replay、delivery
cursor、interaction lease 或 Task-governed foreground PTY target。Shell event
journal 不是 Task EventStore。

## 3. 当前就绪度

| 领域 | 基线状态 | 验收状态 | 通过所需证据 |
| --- | --- | --- | --- |
| Foreground PTY ownership | 已在 Shell Host 实现 | 仅为现有行为 | Attached/degraded mode 下的 regression evidence |
| Direct local shell mode | 已实现 | 仅为现有行为 | Gateway outage 和 rollback test |
| Gateway attachment client | 不存在 | **NOT IMPLEMENTED** | Versioned API contract 和 integration test |
| 持久 attachment identity | 不存在 | **NOT IMPLEMENTED** | `AttachmentId` 与 lease persistence test |
| Task projection presenter | Terminal renderer 由 Shell state 驱动 | **NOT IMPLEMENTED** | Golden projection-to-card fixture |
| Replay cursor | Shell event cursor 只在内存中 | **NOT IMPLEMENTED** | Restart/reconnect cursor test |
| Attach/detach lifecycle | 不存在 | **NOT IMPLEMENTED** | State-machine integration test |
| Interaction lease | 不存在 | **NOT IMPLEMENTED** | Multi-client claim/expiry test |
| Gateway prompt/cancel path | Provider call 由 Shell 拥有 | **NOT IMPLEMENTED** | Idempotent Task command test |
| Gateway approval/question path | Local card state 具有权威性 | **NOT IMPLEMENTED** | Versioned command/replay test |
| Governed foreground PTY target | Approved handoff 是本地 Shell 逻辑 | **NOT IMPLEMENTED** | Broker permit 与 target lifecycle test |
| Task/Shell ID separation | 尚未跨进程类型化约束 | **NOT IMPLEMENTED** | Cross-ID negative test |
| Daemon restart recovery | 不存在 | **NOT IMPLEMENTED** | PTY-continuity 与 projection-replay test |

必须保留现有 PTY 与 direct-mode 行为，但它们本身不满足 Phase 2 attachment
验收。

## 4. Exit Criteria

| ID | 标准 | 必需证明 |
| --- | --- | --- |
| SH-01 | `ShellSessionId`、`TaskId` 与 `AgentSessionId` 具有独立类型和生命周期 | API review 与 cross-ID compile/negative test |
| SH-02 | cosh-shell 继续作为 foreground PTY descriptor 的唯一 owner | Ownership review 与 process-lifecycle test |
| SH-03 | Gateway 关闭时 direct local command 仍可运行 | Raw shell integration 和 manual TTY evidence |
| SH-04 | Gateway failure 永不阻塞 PTY input/output relay | Disconnect 与 saturation test |
| SH-05 | Attach 返回 snapshot、replay 与稳定 cursor | Gateway fake integration test |
| SH-06 | Detach 只停止 Presentation，不取消 Task、Run、Agent session 或 PTY | 对每个非效果的生命周期测试 |
| SH-07 | Shell 退出释放 attachment，而持久 Task 继续 | Process-exit integration test |
| SH-08 | 重连从最后连续 cursor 开始，对每个 projection item 只应用一次 | Restart 与 duplicate-delivery test |
| SH-09 | Cursor 过期时从 snapshot 重建 card，不 replay side effect | Retention-gap test |
| SH-10 | Approval 与 question decision 只有在 Gateway 接受后才有权威性 | Lost-response、stale-version 与 replay test |
| SH-11 | 并发 interactive Client 由带 expiry 的 lease 限制 | Shell/Web contention test |
| SH-12 | Agent command 只有具备 target-bound Broker permit 才能进入 foreground PTY | Permit forgery、expiry 与 digest mismatch test |
| SH-13 | ACP terminal 不会隐式 attach 到 foreground PTY | Runtime-to-target routing negative test |
| SH-14 | PTY busy、timeout、disconnect 和 owner exit 产生 typed non-success outcome | Target lifecycle test |
| SH-15 | 不可信 projection content 不能注入 Terminal control sequence | Rendering adversarial fixture |
| SH-16 | 现有 job control、signal、resize、alternate screen 和 terminal recovery 保持不变 | Shell Host suite 加明确请求的 manual TTY gate |
| SH-17 | 禁用 attachment 可恢复当前 direct/cosh-core 路径 | Rollback smoke test |

Shell Attachment 模块退出时必须满足所有标准。

## 5. 必需自动化证据

实现报告必须包含 candidate 完整 SHA、准确 command、test count 和 failure/skip。
至少覆盖仓库最接近的 Shell 分层：

```text
cargo test --package cosh-shell --lib
cargo test --package cosh-shell --test logic
cargo test --package cosh-shell --test protocol
cargo test --package cosh-shell --test shell_host -- --test-threads=4
crates/cosh-shell/scripts/check-layout.sh
```

新增 targeted attachment test 可以使用其他已批准 target，但修改 `shell_host/`
时不能替代 PTY regression。本次仅文档报告没有运行上述命令。

## 6. 必需手工证据

Release 前必须明确请求 manual TTY gate，并验证：

- Direct bash/zsh command entry 与 interactive program；
- Attach、live update、detach 和 reattach；
- Gateway stop/restart 时 foreground shell 仍可使用；
- Approval 与 question capture，包括 cancellation 和 stale decision；
- `Ctrl+C`、resize、alternate screen、foreground job 和 Terminal restoration；
- 带可见 origin 和 result 的 Agent-permitted PTY handoff；
- Shell 退出后 Task 在其他 Presenter 中继续。

报告必须标识准确 commit 与环境，并脱敏 workspace、command output 和 credential。
本次没有执行该测试。

## 7. 剩余 Blocker

- 必须具备 Phase 1 Gateway API、Task projection、Outbox、command idempotency 与
  Capability Broker。
- 必须在 Phase 0 冻结 Presentation 与 attachment schema。
- Shell 与 Web 的 interaction-lease 默认归属尚未决定。
- Cursor cache retention 与保护需要批准 policy。
- 实现前需要为当前 `InlineState` field 的迁移 ownership 制定并评审文件计划。

## 8. 验收决定

**NOT IMPLEMENTED / NOT ACCEPTED。** 当前仅通过源码检查确认现有 PTY 功能。
Phase 2 Shell Attachment 验收要求在同一 candidate revision 上提供 SH-01 至
SH-17 全部证据，并在实现就绪时完成明确请求的 manual TTY gate。
