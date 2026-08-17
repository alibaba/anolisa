# Phase 1 Gateway API Acceptance Baseline

[中文版](acceptance_zh.md) | [Design](design.md)

## Baseline result

**Overall: PARTIAL candidate implementation based on
`e90d9d9402c7fa1c8122267eb4e075c0adda51f5`; Phase 1 remains NOT ACCEPTED.** The candidate adds a
bounded local Unix daemon/client with peer-UID authentication, handler ports, durable Task
submit/get/events/cancel/retry/append-input/resolve-approval, Outbox scheduling, and restart
convergence. Production `serve` admits only `core/gateway-brokered-v1` with a contained task-only
inventory whose only production tool is `ask_user_question`; no production `ExecutionTarget` or
checkpoint/ws-ckpt dependency is wired. `doctor` and `run` remain explicitly ungoverned ACP
interoperability.
Remote/multi-tenant identity, channel adapters, accepted real Codex/Claude evidence, and manual
Terminal validation remain absent.

Phase 1 is installation-scoped and single-user. The durable `InstallationId` plus authenticated
local peer UID is the v1 authorization boundary. `TenantId`, cross-tenant authority, and remote
identity are future v2 work.

## Result vocabulary

| Result | Meaning |
| --- | --- |
| PASS | Evidence at the pinned commit satisfies the criterion. |
| FAIL | An implementation exists but contradicts the criterion. |
| NOT IMPLEMENTED | The required production path does not exist. |
| BLOCKED | Verification cannot proceed until an identified external decision or dependency lands. |

## Evidence inspected

- Planning baseline: `e90d9d9402c7fa1c8122267eb4e075c0adda51f5`.
- [`cosh-types/output.rs`](../../../../../crates/cosh-types/src/output.rs) defines the current CLI
  response envelope.
- [`cosh-cli/main.rs`](../../../../../crates/cosh-cli/src/main.rs) dispatches directly to current
  command modules.
- [`cosh-core/protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs) defines an internal
  shell/core JSONL protocol.
- [`cosh-core/session_control.rs`](../../../../../crates/cosh-core/src/session_control.rs) manages
  provider sessions, not Tasks.
- Candidate source adds a private versioned local API, daemon, typed client, installed CLI route,
  and SQLite-backed Task projections without a remote listener.
- [`daemon/handler.rs`](../../../../../crates/cosh-gateway/src/daemon/handler.rs) depends only on a
  resolved actor, admission values, `TaskCommandPort`, and `TaskProjectionPort`.
- [`daemon/tests.rs`](../../../../../crates/cosh-gateway/src/daemon/tests.rs) covers the frozen wire,
  response loss, exact frame bounds, forbidden handler imports, and the 250 ms connection quantum.

## Acceptance matrix

| ID | Criterion | Baseline | Evidence or missing artifact |
| --- | --- | --- | --- |
| GWA-001 | A versioned bounded local API accepts typed Task commands. | PASS for local v1 surface | Frozen Gateway wire v1 covers every enabled command, including exact pending input append and fenced retry, plus exact/oversized frame tests. Remote ingress remains Phase 2. |
| GWA-002 | Transport identity overrides any untrusted actor body. | PARTIAL | Requests carry no actor; `InstallationId` plus Unix peer UID is authoritative and forged identity fields fail. Tenant/remote identity is intentionally future v2. |
| GWA-003 | Handler code has no OS, PTY, process-spawn, Agent, store, scheduler, or Runtime capability. | PASS | `daemon/handler.rs` imports contracts and its two ports only; a source-boundary test rejects forbidden dependencies. |
| GWA-004 | Every enabled mutation is sent through `TaskCommandPort`. | PASS | Submit, cancel, retry, append-input, and resolve-approval use the command port; get/events use `TaskProjectionPort`. |
| GWA-005 | `TaskCoordinator` is the only Task aggregate writer. | PARTIAL | Local service and scheduler settlement use the coordinator; a final ownership audit across future adapters remains. |
| GWA-006 | Same idempotency key and digest replay the original receipt. | PASS | The raw Unix response-loss fixture drops the first submit response, retries, and returns the durable original; cancel replay is also covered. |
| GWA-007 | Same idempotency key with a different digest fails deterministically. | PASS | The end-to-end local API fixture returns non-recoverable `idempotency_conflict`. |
| GWA-008 | Task reads and bounded event pages are authorized. | PARTIAL | Installation-derived local actors and peer UID gate reads, foreign actors receive not-found, and pages are bounded. Tenant authorization is not implemented or claimed. |
| GWA-009 | Approval resolution cannot create or widen a permit. | PARTIAL | The asynchronous endpoint commits a bound terminal approval. Generic Permit/Execution contracts remain future foundations; no production ExecutionTarget or checkpoint loop is enabled. Universal presentation/Broker coverage remains. |
| GWA-010 | Outbox delivery tolerates duplicate send and restart. | PARTIAL | Scheduler claim/reclaim/ack and stable Delivery IDs are retained in the task-only slice; no checkpoint-specific execution or result replay is claimed. Universal delivery, remote paths, and power-loss evidence remain open. |
| GWA-011 | Existing shell/core JSONL is not exposed as Gateway API. | PASS | It remains scoped to runtime code. |
| GWA-012 | Existing compatibility behavior remains available when daemon is disabled. | PASS | Standalone Shell remains the rollback path. `doctor`/`run` are independent, ungoverned ACP interop and are not a governed claim. |
| GWA-013 | Remote listeners are disabled in Phase 1. | PASS for source slice | Only a local Unix listener exists. |
| GWA-014 | Phase 1 identity authority and future cross-channel boundary are selected. | PASS for scope decision | v1 uses `InstallationId` plus local peer identity. Remote channels, `TenantId`, and multi-tenant authorization require v2. |

## Required fixtures and commands for implementation acceptance

The implementation report must retain these artifacts under the eventual Gateway test owner:

| Fixture/artifact | Purpose |
| --- | --- |
| `gateway-wire-v1.json` golden corpus | Every enabled command plus strict invalid, oversized, and version tests. |
| `idempotency-replay` crash fixture | Commit a command, drop response, retry, compare receipt. |
| `forged-actor` fixture | Prove body identity cannot override peer/channel identity. |
| `handler-boundary` dependency test | Fail on imports of execution, PTY, process, store, or Agent bridge. |
| `outbox-redelivery` fixture | Restart between send and acknowledgment and prove stable Delivery ID. |

Expected scoped commands after code exists are:

```bash
cargo test --package cosh-gateway gateway_api
cargo test --package cosh-gateway gateway_contract
cargo test --package cosh-gateway-contracts gateway_schema
```

The focused daemon suite covers peer/server UID authentication, installation binding, all enabled
wire commands, exact maximum and oversized framing, SQL event pages, strict fields, response-loss
replay and digest conflict, cancellation, safe stale sockets, handler import boundaries, and
idle/partial-frame clients timing out after one 250 ms admission quantum while scheduler progress
and the following valid request continue. Reproduce it with
`cargo test --locked --package cosh-gateway daemon --no-fail-fast`.

No accepted real Codex/Claude, ECS, remote transport, manual Terminal, complete
crash-after-commit matrix, or screenshot evidence is claimed here. The
task-only inventory exposes only `ask_user_question`; no production
ExecutionTarget or checkpoint/ws-ckpt path is evidence in this report.

## Exit criteria

Phase 1 Gateway API is accepted only when:

1. GWA-001 through GWA-013 are PASS; GWA-014 records the deliberately installation-scoped owner
   decision.
2. The handler-boundary test proves a Gateway handler cannot execute OS work.
3. Crash/retry fixtures demonstrate durable idempotency and transactional outbox behavior.
4. Security review covers peer credentials, installation/actor binding, target substitution, replay,
   resource limits, redaction, and approval authorization.
5. The acceptance report records the exact commit, commands, test counts, artifacts, and untested
   external-channel paths.

## Current risks

- Reusing `CoshResponse<T>` directly could conflate CLI execution with asynchronous Task receipt.
- Reusing the shell/core JSONL contract would leak runtime assumptions into public ingress.
- Adding channel handlers before Task idempotency would make weak-network retries unsafe.
- Treating the v1 installation boundary as cross-tenant authority would be a security error;
  multi-tenant semantics require a new version and review.
