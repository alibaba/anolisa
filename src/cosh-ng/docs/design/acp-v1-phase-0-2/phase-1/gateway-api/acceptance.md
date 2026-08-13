# Phase 1 Gateway API Acceptance Baseline

[中文版](acceptance_zh.md) | [Design](design.md)

## Baseline result

**Overall: NOT IMPLEMENTED at `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`.** Existing JSON
envelopes and correlated control messages are useful inputs, but no Gateway API, Task command
port, ingress identity, durable idempotency, or projection delivery path exists.

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

- Baseline: `git rev-parse HEAD` returned
  `6c115aefe04ace0d169a24fa7cd55ad7c1befa52`.
- [`cosh-types/output.rs`](../../../../../crates/cosh-types/src/output.rs) defines the current CLI
  response envelope.
- [`cosh-cli/main.rs`](../../../../../crates/cosh-cli/src/main.rs) dispatches directly to current
  command modules.
- [`cosh-core/protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs) defines an internal
  shell/core JSONL protocol.
- [`cosh-core/session_control.rs`](../../../../../crates/cosh-core/src/session_control.rs) manages
  provider sessions, not Tasks.
- Repository search found no `GatewayApi`, `IngressPort`, or `TaskCommandPort` implementation.

## Acceptance matrix

| ID | Criterion | Baseline | Evidence or missing artifact |
| --- | --- | --- | --- |
| GWA-001 | A versioned bounded local API accepts typed Task commands. | NOT IMPLEMENTED | No daemon/API module. |
| GWA-002 | Transport identity overrides any untrusted actor body. | NOT IMPLEMENTED | No identity resolver or ingress envelope. |
| GWA-003 | Handler code has no OS, PTY, process-spawn, Agent, or store capability. | NOT IMPLEMENTED | No handler boundary to inspect. |
| GWA-004 | Every mutation is sent through `TaskCommandPort`. | NOT IMPLEMENTED | Port absent. |
| GWA-005 | `TaskCoordinator` is the only Task aggregate writer. | NOT IMPLEMENTED | Task aggregate absent. |
| GWA-006 | Same request and digest replay the original receipt. | NOT IMPLEMENTED | No durable idempotency table. |
| GWA-007 | Same request with a different digest fails deterministically. | NOT IMPLEMENTED | No request ledger. |
| GWA-008 | Task reads and bounded event pages are tenant-authorized. | NOT IMPLEMENTED | No projection/event API. |
| GWA-009 | Approval resolution cannot create or widen a permit. | NOT IMPLEMENTED | Approval endpoint and Broker absent. |
| GWA-010 | Outbox delivery tolerates duplicate send and restart. | NOT IMPLEMENTED | No outbox consumer. |
| GWA-011 | Existing shell/core JSONL is not exposed as Gateway API. | PASS | It remains scoped to runtime code. |
| GWA-012 | Existing CLI behavior remains available when daemon is disabled. | PASS | No daemon integration exists yet. |
| GWA-013 | Remote listeners are disabled in Phase 1. | PASS | No listener exists; retain this property. |
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

These commands were **not run** because the candidate package has no Gateway API implementation
or matching test targets. The existing package-level suite validates other candidate slices only;
documentation checks validate this still-unimplemented module's links and bilingual parity.

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
