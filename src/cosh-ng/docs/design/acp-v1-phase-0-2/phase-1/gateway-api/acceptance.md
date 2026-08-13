# Phase 1 Gateway API Acceptance Baseline

[中文版](acceptance_zh.md) | [Design](design.md)

## Baseline result

**Overall: PARTIAL candidate implementation based on upstream
`e3763b001c91f3c13dc6afbd57aac924162e9f59`; Phase 1 remains NOT ACCEPTED.** The candidate adds a
bounded local Unix daemon/client slice with peer-UID authentication and durable Task
submit/get/events/cancel. Runtime scheduling, approval resolution, Outbox delivery, restart
reconciliation, remote identity, channel adapters, and real-provider evidence remain absent.

This report records readiness before implementation. It must not be interpreted as a Phase 1
acceptance pass.

## Result vocabulary

| Result | Meaning |
| --- | --- |
| PASS | Evidence at the pinned commit satisfies the criterion. |
| FAIL | An implementation exists but contradicts the criterion. |
| NOT IMPLEMENTED | The required production path does not exist. |
| BLOCKED | Verification cannot proceed until an identified external decision or dependency lands. |

## Evidence inspected

- Upstream baseline: `e3763b001c91f3c13dc6afbd57aac924162e9f59`.
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

## Acceptance matrix

| ID | Criterion | Baseline | Evidence or missing artifact |
| --- | --- | --- | --- |
| GWA-001 | A versioned bounded local API accepts typed Task commands. | PARTIAL | Local submit/get/events/cancel and bounded framing exist; approval/append/retry and frozen golden corpus remain. |
| GWA-002 | Transport identity overrides any untrusted actor body. | PARTIAL | Requests carry no actor and Unix peer UID is authoritative; tenant/remote identity resolution remains. |
| GWA-003 | Handler code has no OS, PTY, process-spawn, Agent, or store capability. | PARTIAL | Handler has no Runtime/PTY/OS execution; current daemon owns the local store directly, so the target port split is incomplete. |
| GWA-004 | Every mutation is sent through `TaskCommandPort`. | PARTIAL | Mutations use one daemon service path, but a separately enforced `TaskCommandPort` boundary remains. |
| GWA-005 | `TaskCoordinator` is the only Task aggregate writer. | PARTIAL | Local service serializes Task writes; scheduling/recovery coordinator is absent. |
| GWA-006 | Same request and digest replay the original receipt. | PARTIAL | Durable command receipts back submit/cancel; crash-after-commit retry evidence remains. |
| GWA-007 | Same request with a different digest fails deterministically. | PARTIAL | Store conflict behavior exists; end-to-end local API fixture remains. |
| GWA-008 | Task reads and bounded event pages are tenant-authorized. | PARTIAL | Peer UID gates Task reads and pages are limited; tenant authorization is not implemented. |
| GWA-009 | Approval resolution cannot create or widen a permit. | NOT IMPLEMENTED | Approval endpoint and Broker absent. |
| GWA-010 | Outbox delivery tolerates duplicate send and restart. | NOT IMPLEMENTED | No outbox consumer. |
| GWA-011 | Existing shell/core JSONL is not exposed as Gateway API. | PASS | It remains scoped to runtime code. |
| GWA-012 | Existing CLI behavior remains available when daemon is disabled. | PASS for source slice | Existing `doctor` and `run` routes remain independent of `serve`/`task`. |
| GWA-013 | Remote listeners are disabled in Phase 1. | PASS for source slice | Only a local Unix listener exists. |
| GWA-014 | Cross-channel identity authority is selected. | BLOCKED | Product/security owner decision remains open. |

## Required fixtures and commands for implementation acceptance

The implementation report must retain these artifacts under the eventual Gateway test owner:

| Fixture/artifact | Purpose |
| --- | --- |
| `gateway-v1/*.json` golden corpus | Valid, invalid, oversized, unknown-version requests and responses. |
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

The Stage 6 commit containing this report passed on Rust 1.88:

- `cargo +1.88 test -p cosh-gateway --no-fail-fast`: 126 library, 4 binary,
  and 7 installed-CLI tests passed; 0 failed.
- `cargo +1.88 clippy -p cosh-gateway --all-targets -- -D warnings` passed.
- `cargo +1.88 doc -p cosh-gateway --no-deps` passed.
- Focused fixtures cover peer/server UID authentication, installation binding,
  bounded framing and SQL event pages, strict fields, replay, queued cancel,
  safe stale sockets, and installed CLI parsing.
- A local built-binary smoke completed `serve`, `task submit`, one-page
  `events`, queued `cancel`, and SIGINT socket cleanup through the Unix API.

No real provider, ECS, remote transport, manual Terminal, crash-after-commit,
audit-evidence sink, or screenshot evidence is claimed here.

## Exit criteria

Phase 1 Gateway API is accepted only when:

1. GWA-001 through GWA-013 are PASS; GWA-014 has a recorded decision or a deliberately local-only
   scope with owner approval.
2. The handler-boundary test proves a Gateway handler cannot execute OS work.
3. Crash/retry fixtures demonstrate durable idempotency and transactional outbox behavior.
4. Security review covers peer credentials, tenant/actor binding, target substitution, replay,
   resource limits, redaction, and approval authorization.
5. The acceptance report records the exact commit, commands, test counts, artifacts, and untested
   external-channel paths.

## Current risks

- Reusing `CoshResponse<T>` directly could conflate CLI execution with asynchronous Task receipt.
- Reusing the shell/core JSONL contract would leak runtime assumptions into public ingress.
- Adding channel handlers before Task idempotency would make weak-network retries unsafe.
- Treating a local single-user deployment as identity-free would make later remote migration a
  breaking security change.
