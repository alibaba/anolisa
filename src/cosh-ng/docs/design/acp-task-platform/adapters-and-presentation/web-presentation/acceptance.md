# Phase 2 Web and Presentation Acceptance Report

[中文版](acceptance_zh.md)

Related design: [Web and Presentation design](design.md).

## 1. Report scope

- Baseline reviewed: `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`
- Review date: 2026-08-12
- Change type: planning documentation only
- Implementation acceptance: **NOT ACCEPTED**

This report defines future evidence. It does not claim that a Web surface,
Gateway API, Projection, Outbox, or delivery receipt exists.

## 2. Baseline evidence

The baseline has terminal-specific rendering and process-local shell state. It
has no Web component, browser transport, Gateway HTTP API, Task projection,
transactional Outbox, ordered presentation stream, delivery receipt, or
multi-client attachment state.

The cosh-core JSONL stream and the shell `events.jsonl` journal are private
runtime/evidence formats. Neither is accepted as a browser API or Web replay
source.

## 3. Current readiness

| Area | Baseline status | Acceptance status | Evidence needed to pass |
| --- | --- | --- | --- |
| Web-safe projection schema | Not present | **NOT IMPLEMENTED** | Versioned schema and golden fixtures |
| Gateway HTTP command/query API | Not present | **NOT IMPLEMENTED** | Authenticated contract tests |
| Ordered event stream | Not present | **NOT IMPLEMENTED** | Cursor/reconnect integration tests |
| Transactional Projection + Outbox | Not present | **NOT IMPLEMENTED** | Crash-boundary transaction tests |
| Delivery worker | Not present | **NOT IMPLEMENTED** | Lease, retry, ordering, and duplicate tests |
| Delivery receipt | Not present | **NOT IMPLEMENTED** | Monotonic per-client watermark tests |
| Attach/detach record | Not present | **NOT IMPLEMENTED** | Lifecycle and expiry tests |
| Browser reducer | Not present | **NOT IMPLEMENTED** | Duplicate/gap/reset golden tests |
| Web approval/question controls | Not present | **NOT IMPLEMENTED** | Version/idempotency and sensitive-input tests |
| Interaction lease | Not present | **NOT IMPLEMENTED** | Shell/Web contention tests |
| Bounded execution output view | Not present | **NOT IMPLEMENTED** | Auth, expiry, redaction, and bounds tests |
| Weak-network recovery | Not present | **NOT IMPLEMENTED** | Offline/restart/slow-client test harness |
| Web security controls | Not present | **NOT IMPLEMENTED** | Threat review and adversarial tests |
| Direct-boundary enforcement | No Web path exists | **NOT IMPLEMENTED** | Proof Web cannot call ACP/PTY/store/target directly |

## 4. Exit criteria

| ID | Criterion | Required proof |
| --- | --- | --- |
| WEB-01 | Browser uses only the authorized Gateway API and Presentation Port | Dependency and route review plus negative tests |
| WEB-02 | Web never consumes ACP JSON-RPC or cosh-core JSONL | Boundary test and dependency audit |
| WEB-03 | Projection schema is versioned, bounded, redacted, and runtime-neutral | Schema fixtures and payload-limit tests |
| WEB-04 | Task event, projection update, Outbox record, and idempotency result commit atomically | Pre/post-commit crash tests |
| WEB-05 | Outbox publication is ordered per Task and safe to repeat | Multi-worker lease and duplicate tests |
| WEB-06 | Attach has an atomic snapshot/stream boundary | Concurrent-update race test |
| WEB-07 | Reconnect replays after the highest contiguous applied cursor | Disconnect/reconnect integration test |
| WEB-08 | Expired cursor returns a typed reset and authorized fresh snapshot | Retention-gap test |
| WEB-09 | Receipts advance monotonically per actor/client/attachment/Task and reject gaps | Receipt contract tests |
| WEB-10 | A receipt never means user approval, viewing, or command acceptance | Domain/API review and state-transition tests |
| WEB-11 | Lost command responses reconcile by idempotency key and projection | Timeout and duplicate-submit tests |
| WEB-12 | Approval/question commands reject stale, duplicate, unauthorized, and resolved input | Conflict and authorization tests |
| WEB-13 | Multiple viewers cannot accidentally become concurrent writers | Interaction-lease contention/expiry tests |
| WEB-14 | Slow or offline clients cannot create unbounded server queues | Backpressure and later-replay tests |
| WEB-15 | Gateway and delivery-worker restarts do not lose committed visible state | Restart durability tests |
| WEB-16 | Projection and output rendering resists HTML/Markdown/URL/control injection | Adversarial rendering suite |
| WEB-17 | Secrets and raw credentials do not reach projection, logs, URLs, receipts, or local storage | Data-flow review and secret-canary tests |
| WEB-18 | Authorization revocation closes streams and prevents replay/commands | Revocation end-to-end test |
| WEB-19 | Execution output uses authorized expiring bounded references | Access, expiry, and size tests |
| WEB-20 | Web can be disabled without changing Shell direct mode or Task durability | Rollback smoke test |

All WEB-01 through WEB-20 criteria are mandatory for Phase 2 exit.

## 5. Required automated evidence

The future implementation report must record:

- full candidate commit SHA and exact targeted commands/test counts;
- schema versions for Gateway, ProjectionEnvelope, snapshot, and receipt;
- database and transaction backend used by the tests;
- Outbox claim lease, retry, queue, payload, and retention limits;
- stream transport and proxy assumptions;
- authorization, CSRF/origin, token/cookie, and redaction test coverage;
- deterministic weak-network cases: drop, delay, duplicate, reordering, sleep,
  restart, and slow consumer;
- untested browsers, remote deployment modes, and optional transports.

Expected targeted test groups, with final commands chosen by implementation
ownership:

```text
<gateway contract tests>
<task transaction and outbox tests>
<presentation stream and receipt tests>
<web reducer/component tests>
<weak-network end-to-end tests>
```

No code suites were run for this documentation-only report because those
modules are not implemented.

## 6. Manual and deployment evidence

Before a remotely exposed release, an explicitly requested manual/browser gate
must verify attach, replay, live updates, approvals, cancellation, multi-device
receipts, offline recovery, authorization revocation, and responsive rendering.
Deployment review must cover the actual reverse proxy and authentication
mode. No browser, public-network, ECS, provider, or screenshot validation was
performed here.

## 7. Remaining blockers

- Phase 0 projection, cursor, attachment, identity, redaction, and error
  contracts must be accepted.
- Phase 1 Gateway, Task Plane, transactional EventStore/Projection/Outbox, and
  authorization paths must exist.
- Stream binding and actual deployment authentication are not yet selected.
- Event, output, and receipt retention limits need approved values.
- Viewer/operator role boundaries and interaction-lease granularity remain
  open.

## 8. Acceptance decision

**NOT IMPLEMENTED / NOT ACCEPTED.** The module can exit Phase 2 only after
WEB-01 through WEB-20 pass on one candidate revision and any requested
deployment/manual gate records sanitized evidence.
