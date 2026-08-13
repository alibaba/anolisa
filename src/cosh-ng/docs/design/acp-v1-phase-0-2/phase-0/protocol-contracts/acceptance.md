# Phase 0 Protocol Contracts Acceptance Report

[中文版](acceptance_zh.md) | [Design](design.md) |
[Planning set](../../README.md)

## Baseline result

**The leaf-contract slice is accepted; Phase 0 implementation exit is not.**
This report covers the implementation worktree based on
`6c115aefe04ace0d169a24fa7cd55ad7c1befa52`. The worktree also contains a Task
reducer, SQLite Task storage, Runtime primitives, and a process-local
Capability Broker slice. It does not claim complete schemas, fixtures,
coordinator/port integration, durable Broker authority, or a complete ACP bridge.

The new side-effect-free package provides neutral Task, Runtime, Capability,
Approval, execution, header, and error types. Its deserializers validate typed
IDs, schema version and envelope kind, bounded text, opaque values, digests,
and error codes. Individual scalar fields are bounded; aggregate `Vec` and
envelope collection totals are not yet capped and therefore do not satisfy a
complete bounded-input claim.

## Evidence reviewed

| Source | Verified baseline behavior |
| --- | --- |
| [`protocol.rs`](../../../../../crates/cosh-core/src/protocol.rs#L9), symbols `CONTROL_PROTOCOL_VERSION`, `InputMessage`, `OutputMessage` | Exact product-specific shell/core protocol version `1`; not ACP |
| [`AgentAdapter`](../../../../../crates/cosh-shell/src/adapter/mod.rs#L87), [`AgentRunHandle`](../../../../../crates/cosh-shell/src/adapter/mod.rs#L107), and [`AgentEvent`](../../../../../crates/cosh-shell/src/types/mod.rs#L402) | Shell-local Agent lifecycle abstraction exists |
| [`session.rs`](../../../../../crates/cosh-core/src/session.rs#L83), symbols `PersistedSession`, `SessionError` | Versioned provider-session envelope and typed errors exist |
| [`types/audit.rs`](../../../../../crates/cosh-shell/src/types/audit.rs#L29), symbol `AuditIdentity` | Multiple correlation strings exist, without Task or Execution identity |
| [`cosh-gateway-contracts`](../../../../../crates/cosh-gateway-contracts/src/lib.rs) and its [manifest](../../../../../crates/cosh-gateway-contracts/Cargo.toml) | Side-effect-free leaf crate depends only on workspace `serde`, `thiserror`, and `uuid`; no ACP, transport, async, storage, or OS dependency |
| [`task.rs`](../../../../../crates/cosh-gateway-contracts/src/task.rs) and [`runtime.rs`](../../../../../crates/cosh-gateway-contracts/src/runtime.rs) | Versioned command/event envelopes and neutral Task/Runtime payloads are public and documented |
| [`capability.rs`](../../../../../crates/cosh-gateway-contracts/src/capability.rs) and [`error.rs`](../../../../../crates/cosh-gateway-contracts/src/error.rs) | Capability request/decision/permit and bounded machine-readable errors are implemented |
| [`aggregate.rs`](../../../../../crates/cosh-gateway/src/task/aggregate.rs) and [`task_store.rs`](../../../../../crates/cosh-gateway/src/storage/task_store.rs) | Task transitions, revision checks, terminal guards, transactional event/projection/receipt/Outbox writes, and idempotency replay/conflict tests exist |
| [`capability/broker.rs`](../../../../../crates/cosh-gateway/src/capability/broker.rs) and [`capability/memory.rs`](../../../../../crates/cosh-gateway/src/capability/memory.rs) | Broker-facing policy branches and process-local atomic single-use permit checks exist; approval and authority are not durable |
| [Implemented runtime contract](../../../../../docs/design/runtime-contracts.md) | Existing JSONL negotiation and process path remain compatibility inputs |

Review covered source, dependency direction, rustdoc, serialization tests, and
targeted package validation. It did not call a provider, access ECS, or mutate
the host.

## Acceptance matrix

| ID | Requirement | Baseline | Evidence required to pass |
| --- | --- | --- | --- |
| PC-01 | Neutral Task command and event types exist in the accepted side-effect-free owner | Partial | Rust types, rustdoc, and dependency direction pass; ownership ADR and schema fixtures remain |
| PC-02 | Runtime Port types contain no ACP, cosh-core, Shell, HTTP, or channel type | Partial | Neutral Runtime command/event types pass; behavioral port and API review remain |
| PC-03 | Capability request, permit, approval, and execution outcomes are typed | Partial | Public serde types and eight Broker-facing tests pin target/descriptor/complete-operation digest/scope before policy; trusted canonicalizer tests, golden schemas, durable approval, and execution-result lifecycle remain |
| PC-04 | Product schema versions are independent from ACP and core versions | Partial | Explicit schema constant and fail-closed version/type tests pass; compatibility manifest remains |
| PC-05 | Task reducer enforces monotonic revisions and one terminal Run event | Partial | Critical revision-gap and terminal-guard tests pass; duplicate/reorder property matrix remains |
| PC-06 | Command idempotency specifies same-key/same-digest replay and conflict | Partial | SQLite integration replays the same actor/key/digest and rejects a changed digest; authenticated ingress scope and fixture corpus remain |
| PC-07 | Cancellation persists intent and resolves completion races deterministically | Partial | Cancellation intent/terminal facts and reducer guards exist; fake-runtime completion-race fixtures remain |
| PC-08 | Errors are bounded, redacted, stable, and machine-readable | Partial | Scalar code/message construction and deserialization pass; aggregate collection caps and secret-scanner adversarial fixtures remain |
| PC-09 | ACP v1 baseline and capability negotiation are represented in fixtures | Missing | Official SDK-generated initialize/session fixtures |
| PC-10 | Existing shell/core JSONL v1 remains compatible | Ready as baseline only | Existing protocol suite plus new CoshCore bridge fixtures |
| PC-11 | Unknown versions and unsupported capabilities fail closed | Partial | Unknown Gateway schema version and envelope-kind mismatch fail closed; ACP fake-Agent negative tests remain |
| PC-12 | English and Chinese design/acceptance pairs remain equivalent | Ready for this change after doc checks | Parity review recorded below |

`Partial` means the leaf source or an existing precedent covers only part of
the requirement; the remaining evidence in the last column is still required.

## Required fixture inventory

Implementation cannot exit Phase 0 until the repository contains versioned
fixtures equivalent to:

```text
fixtures/gateway-contracts/v1/
  gateway-command-create-task.json
  gateway-command-idempotency-conflict.json
  task-event-run-lifecycle.jsonl
  task-event-approval-execution.jsonl
  runtime-command-prompt.json
  runtime-event-message-tool-permission.jsonl
  capability-request.json
  execution-permit.json
  contract-error.json
  malformed/
    unknown-schema-version.json
    oversized-content.json
    cross-task-correlation.json
fixtures/acp/v1/
  initialize-minimal.jsonl
  initialize-capabilities.jsonl
  prompt-cancel.jsonl
  permission-terminal.jsonl
fixtures/cosh-core-bridge/v1/
  initialize-and-turn.jsonl
  approval-and-host-execution.jsonl
```

Fixture paths remain a proposed artifact layout; they are not provided by the
leaf-contract slice.

## Required validation commands

The remaining implementation acceptance must record these equivalent commands
and counts:

```bash
cargo test --package cosh-gateway-contracts
cargo test --package cosh-gateway task_reducer
cargo test --package cosh-gateway --test contract_fixtures
cargo test --package cosh-shell --test protocol
```

Also required:

- JSON Schema validation for every positive and malformed fixture;
- dependency graph evidence showing domain contracts do not depend on ACP or
  transport crates;
- a generated compatibility manifest recording ACP wire `1`, actual ACP SDK
  package version, Gateway schema `1`, and core control protocol `1` as
  distinct values;
- diff evidence that current shell/core protocol fixtures still pass.

Targeted leaf-crate validation recorded for this slice:

```text
cargo fmt --package cosh-gateway-contracts -- --check
cargo test --locked --package cosh-gateway-contracts
cargo clippy --locked --package cosh-gateway-contracts --all-targets -- -D warnings
cargo doc --locked --package cosh-gateway-contracts --no-deps
cargo tree --locked --package cosh-gateway-contracts --edges normal
result: 6 integration tests passed; unit and doc-test targets passed
dependency result: serde, thiserror, and uuid only
```

## Missing implementation

- No complete Gateway schema/golden-fixture corpus or aggregate collection limits.
- No integrated Task coordinator and behavioral mapping across Runtime,
  Capability Broker, Presentation, and storage ports.
- No durable approval/permit/execution Broker ledger or reconciliation path;
  the current Broker store is process-local.
- The official ACP Rust SDK, codec, and fake-Agent tests now exist at the
  library boundary; the canonical versioned fixture corpus and real-adapter
  evidence remain incomplete.
- No CoshCore Bridge translating current JSONL to neutral events.
- No compatibility manifest or rollout feature flag.

## Exit criteria

Phase 0 protocol contracts pass only when:

1. PC-01 through PC-12 have implementation evidence at one exact commit.
2. All required schemas and fixtures are reviewed and versioned.
3. State and cancellation property tests pass deterministically.
4. ACP fixtures are produced or consumed through the pinned official Rust SDK,
   with `protocolVersion = 1` asserted separately from the SDK version.
5. No external transport or runtime-specific type appears in Task storage.
6. Security review confirms bounded input and secret-free error behavior.
7. The existing shell/core protocol target remains green.
8. Open decisions that affect a public type or schema are closed in an ADR or
   accepted design revision.

## Validation recorded for this slice

- English and Chinese files use the required reciprocal links.
- Command blocks and schema names are identical across languages.
- Relative source links were checked from this directory.
- Markdown whitespace and diff hygiene were checked.
- Targeted formatting, package tests, Clippy, rustdoc, and dependency audit
  passed with the commands recorded above.
- ECS, provider, and host-mutation validation was intentionally skipped because
  this crate has no I/O or host behavior.
