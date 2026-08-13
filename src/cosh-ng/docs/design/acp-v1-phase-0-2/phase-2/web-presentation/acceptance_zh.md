# Phase 2 Web 与 Presentation 验收报告

[English](acceptance.md)

相关设计：[Web 与 Presentation 设计](design_zh.md)。

## 1. 报告范围

- 审计基线：`6c115aefe04ace0d169a24fa7cd55ad7c1befa52`
- 审计日期：2026-08-12
- 变更类型：仅规划文档
- 实现验收：**NOT ACCEPTED**

本报告定义未来证据，不表示 Web surface、Gateway API、Projection、Outbox 或
delivery receipt 已经存在。

## 2. 基线证据

基线包含 Terminal-specific rendering 和 process-local Shell state，没有 Web
component、Browser transport、Gateway HTTP API、Task projection、transactional
Outbox、有序 Presentation stream、delivery receipt 或 multi-client attachment
state。

cosh-core JSONL stream 和 Shell `events.jsonl` journal 是私有 Runtime/evidence
format，均未被接受为 Browser API 或 Web replay source。

## 3. 当前就绪度

| 领域 | 基线状态 | 验收状态 | 通过所需证据 |
| --- | --- | --- | --- |
| Web-safe projection schema | 不存在 | **NOT IMPLEMENTED** | Versioned schema 和 golden fixture |
| Gateway HTTP command/query API | 不存在 | **NOT IMPLEMENTED** | Authenticated contract test |
| Ordered event stream | 不存在 | **NOT IMPLEMENTED** | Cursor/reconnect integration test |
| Transactional Projection + Outbox | 不存在 | **NOT IMPLEMENTED** | Crash-boundary transaction test |
| Delivery worker | 不存在 | **NOT IMPLEMENTED** | Lease、retry、ordering 与 duplicate test |
| Delivery receipt | 不存在 | **NOT IMPLEMENTED** | Monotonic per-client watermark test |
| Attach/detach record | 不存在 | **NOT IMPLEMENTED** | Lifecycle 与 expiry test |
| Browser reducer | 不存在 | **NOT IMPLEMENTED** | Duplicate/gap/reset golden test |
| Web approval/question control | 不存在 | **NOT IMPLEMENTED** | Version/idempotency 与 sensitive-input test |
| Interaction lease | 不存在 | **NOT IMPLEMENTED** | Shell/Web contention test |
| Bounded execution output view | 不存在 | **NOT IMPLEMENTED** | Auth、expiry、redaction 与 bounds test |
| 弱网恢复 | 不存在 | **NOT IMPLEMENTED** | Offline/restart/slow-client test harness |
| Web security control | 不存在 | **NOT IMPLEMENTED** | Threat review 与 adversarial test |
| Direct-boundary enforcement | 尚无 Web 路径 | **NOT IMPLEMENTED** | Web 不能直接访问 ACP/PTY/store/target 的证明 |

## 4. Exit Criteria

| ID | 标准 | 必需证明 |
| --- | --- | --- |
| WEB-01 | Browser 只使用 authorized Gateway API 和 Presentation Port | Dependency/route review 加 negative test |
| WEB-02 | Web 永不消费 ACP JSON-RPC 或 cosh-core JSONL | Boundary test 与 dependency audit |
| WEB-03 | Projection schema 有版本、有界、脱敏且 Runtime-neutral | Schema fixture 与 payload-limit test |
| WEB-04 | Task event、projection update、Outbox record 和 idempotency result 原子提交 | Commit 前后 crash test |
| WEB-05 | Outbox 按 Task 有序发布并允许安全重复 | Multi-worker lease 与 duplicate test |
| WEB-06 | Attach 具有 atomic snapshot/stream boundary | Concurrent-update race test |
| WEB-07 | 重连从最高连续 applied cursor 后 replay | Disconnect/reconnect integration test |
| WEB-08 | Cursor 过期时返回 typed reset 与 authorized fresh snapshot | Retention-gap test |
| WEB-09 | Receipt 按 actor/client/attachment/Task 单调前进并拒绝 gap | Receipt contract test |
| WEB-10 | Receipt 永不表示用户批准、看到内容或 command 被接受 | Domain/API review 与 state-transition test |
| WEB-11 | 丢失的 command response 按 idempotency key 和 projection reconcile | Timeout 与 duplicate-submit test |
| WEB-12 | Approval/question command 拒绝 stale、duplicate、unauthorized 与 resolved input | Conflict 与 authorization test |
| WEB-13 | 多个 viewer 不会意外成为并发 writer | Interaction-lease contention/expiry test |
| WEB-14 | Slow/offline Client 不产生无界 server queue | Backpressure 与 later-replay test |
| WEB-15 | Gateway 与 delivery-worker 重启不丢失已 committed visible state | Restart durability test |
| WEB-16 | Projection 与 output rendering 防御 HTML/Markdown/URL/control injection | Adversarial rendering suite |
| WEB-17 | Secret 和 raw credential 不进入 projection、log、URL、receipt 或 local storage | Data-flow review 与 secret-canary test |
| WEB-18 | Authorization revocation 关闭 stream 并阻止 replay/command | Revocation end-to-end test |
| WEB-19 | Execution output 使用 authorized、expiring、bounded reference | Access、expiry 与 size test |
| WEB-20 | 禁用 Web 不改变 Shell direct mode 或 Task durability | Rollback smoke test |

退出 Phase 2 必须满足 WEB-01 至 WEB-20 全部标准。

## 5. 必需自动化证据

未来实现报告必须记录：

- Candidate 完整 commit SHA 与准确 targeted command/test count；
- Gateway、ProjectionEnvelope、snapshot 与 receipt 的 schema version；
- Test 使用的 database 和 transaction backend；
- Outbox claim lease、retry、queue、payload 与 retention limit；
- Stream transport 和 proxy assumption；
- Authorization、CSRF/origin、token/cookie 与 redaction test coverage；
- 确定性弱网案例，包括 drop、delay、duplicate、reordering、sleep、restart 和
  slow consumer；
- 尚未测试的 Browser、远端 deployment mode 与 optional transport。

预期 targeted test group 如下，最终 command 由实现 owner 决定：

```text
<gateway contract tests>
<task transaction and outbox tests>
<presentation stream and receipt tests>
<web reducer/component tests>
<weak-network end-to-end tests>
```

本次仅文档报告未运行 code suite，因为这些模块尚未实现。

## 6. 手工与 Deployment 证据

远端暴露 release 前，必须明确请求 manual/browser gate，并验证 attach、replay、
live update、approval、cancellation、multi-device receipt、offline recovery、
authorization revocation 与 responsive rendering。Deployment review 必须覆盖
真实 reverse proxy 与 authentication mode。本次没有执行 Browser、public-
network、ECS、provider 或 screenshot validation。

## 7. 剩余 Blocker

- 必须验收 Phase 0 projection、cursor、attachment、identity、redaction 与 error
  contract。
- 必须具备 Phase 1 Gateway、Task Plane、transactional EventStore/Projection/
  Outbox 与 authorization 路径。
- Stream binding 和真实 deployment authentication 尚未选择。
- Event、output 与 receipt retention limit 需要批准具体值。
- Viewer/operator role boundary 与 interaction-lease granularity 尚未确定。

## 8. 验收决定

**NOT IMPLEMENTED / NOT ACCEPTED。** 只有 WEB-01 至 WEB-20 在同一 candidate
revision 全部通过，并且所有被请求的 deployment/manual gate 都记录脱敏证据后，
模块才能退出 Phase 2。
