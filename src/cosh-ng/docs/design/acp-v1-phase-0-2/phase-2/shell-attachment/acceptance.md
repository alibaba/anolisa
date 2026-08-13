# Phase 2 Shell Attachment Acceptance Report

[中文版](acceptance_zh.md)

Related design: [Shell Attachment design](design.md).

## 1. Report scope

- Baseline reviewed: `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`
- Review date: 2026-08-12
- Change type: planning documentation only
- Implementation acceptance: **NOT ACCEPTED**

This is a readiness baseline and future exit gate. No Task attachment behavior
was implemented or live-tested by this documentation change.

## 2. Baseline evidence

The baseline shell owns its PTY and foreground child, preserves native
bash/zsh behavior, routes raw input/output, renders inline cards, and stores
substantial runtime state in `InlineState`. It has a provider session adapter
and a redacted shell event journal.

It does not have a Gateway client, durable Task attachment, projection replay,
delivery cursor, interaction lease, or Task-governed foreground PTY target.
The shell event journal is not a Task EventStore.

## 3. Current readiness

| Area | Baseline status | Acceptance status | Evidence needed to pass |
| --- | --- | --- | --- |
| Foreground PTY ownership | Implemented in shell host | Existing behavior only | Regression evidence under attached/degraded modes |
| Direct local shell mode | Implemented | Existing behavior only | Gateway outage and rollback tests |
| Gateway attachment client | Not present | **NOT IMPLEMENTED** | Versioned API contract and integration tests |
| Durable attachment identity | Not present | **NOT IMPLEMENTED** | `AttachmentId` and lease persistence tests |
| Task projection presenter | Terminal renderer is shell-state driven | **NOT IMPLEMENTED** | Golden projection-to-card fixtures |
| Replay cursor | Shell event cursor is in memory | **NOT IMPLEMENTED** | Restart/reconnect cursor tests |
| Attach/detach lifecycle | Not present | **NOT IMPLEMENTED** | State-machine integration tests |
| Interaction lease | Not present | **NOT IMPLEMENTED** | Multi-client claim/expiry tests |
| Gateway prompt/cancel path | Provider calls are shell-owned | **NOT IMPLEMENTED** | Idempotent Task command tests |
| Gateway approval/question path | Local card state is authoritative | **NOT IMPLEMENTED** | Versioned command/replay tests |
| Governed foreground PTY target | Approved handoff is local shell logic | **NOT IMPLEMENTED** | Broker permit and target lifecycle tests |
| Task/shell ID separation | Types are not enforced across processes | **NOT IMPLEMENTED** | Cross-ID negative tests |
| Daemon restart recovery | Not present | **NOT IMPLEMENTED** | PTY-continuity and projection-replay tests |

Existing PTY and direct-mode behavior must be preserved, but it does not by
itself satisfy Phase 2 attachment acceptance.

## 4. Exit criteria

| ID | Criterion | Required proof |
| --- | --- | --- |
| SH-01 | `ShellSessionId`, `TaskId`, and `AgentSessionId` are distinct types and lifecycles | API review and cross-ID compile/negative tests |
| SH-02 | cosh-shell remains the sole owner of the foreground PTY descriptors | Ownership review and process-lifecycle tests |
| SH-03 | Direct local commands work when Gateway is down | Raw shell integration and manual TTY evidence |
| SH-04 | Gateway failure never blocks the PTY input/output relay | Disconnect and saturation tests |
| SH-05 | Attach returns snapshot, replay, and a stable cursor | Gateway fake integration test |
| SH-06 | Detach stops presentation only and does not cancel Task, Run, Agent session, or PTY | Lifecycle test for every non-effect |
| SH-07 | Shell exit releases attachment while a durable Task continues | Process-exit integration test |
| SH-08 | Reconnect applies each projection item once from the last contiguous cursor | Restart and duplicate-delivery test |
| SH-09 | Expired cursor rebuilds cards from a snapshot without replaying side effects | Retention-gap test |
| SH-10 | Approval and question decisions are authoritative only after Gateway acceptance | Lost-response, stale-version, and replay tests |
| SH-11 | Concurrent interactive clients are bounded by an expiring lease | Shell/Web contention test |
| SH-12 | Agent commands reach the foreground PTY only with a target-bound Broker permit | Permit forgery, expiry, and digest mismatch tests |
| SH-13 | ACP terminals do not implicitly attach to the foreground PTY | Runtime-to-target routing negative test |
| SH-14 | PTY busy, timeout, disconnect, and owner exit produce typed non-success outcomes | Target lifecycle tests |
| SH-15 | Untrusted projection content cannot inject terminal control sequences | Rendering adversarial fixtures |
| SH-16 | Existing job control, signals, resize, alternate screen, and terminal recovery remain intact | Shell host suite plus requested manual TTY gate |
| SH-17 | Disabling attachments restores the current direct/cosh-core paths | Rollback smoke test |

All criteria are mandatory for the Shell Attachment module exit.

## 5. Required automated evidence

The implementation report must include the full candidate SHA, exact commands,
test counts, and failures or skips. At minimum it must cover the repository's
closest shell layers:

```text
cargo test --package cosh-shell --lib
cargo test --package cosh-shell --test logic
cargo test --package cosh-shell --test protocol
cargo test --package cosh-shell --test shell_host -- --test-threads=4
crates/cosh-shell/scripts/check-layout.sh
```

New targeted attachment tests may use another approved target, but they do not
replace the PTY regressions when `shell_host/` changes. These commands were not
run for this documentation-only report.

## 6. Required manual evidence

Before release, an explicitly requested manual TTY gate must verify:

- direct bash/zsh command entry and interactive programs;
- attach, live update, detach, and reattach;
- Gateway stop/restart while the foreground shell stays usable;
- approval and question capture, including cancellation and stale decision;
- `Ctrl+C`, resize, alternate screen, foreground job, and terminal restoration;
- Agent-permitted PTY handoff with visible origin and result;
- shell exit while the Task continues in another presenter.

The report must identify the exact commit and environment and sanitize any
workspace, command output, or credentials. No such test was performed here.

## 7. Remaining blockers

- Phase 1 Gateway API, Task projections, Outbox, command idempotency, and
  Capability Broker must exist.
- Presentation and attachment schemas must be frozen in Phase 0.
- The interaction-lease default between Shell and Web is unresolved.
- Cursor cache retention and protection need an approved policy.
- Migration ownership for current `InlineState` fields needs a reviewed file
  plan before implementation.

## 8. Acceptance decision

**NOT IMPLEMENTED / NOT ACCEPTED.** Existing PTY functionality is confirmed by
source inspection only. Phase 2 Shell Attachment acceptance requires SH-01
through SH-17 evidence on one candidate revision, including the requested
manual TTY gate when implementation is ready.
