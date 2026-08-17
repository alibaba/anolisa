# Phase 0 Identity and Correlation Acceptance Report

[中文版](acceptance_zh.md) | [Design](design.md) |
[Planning set](../../README.md)

## Baseline result

**The typed leaf identity slice is accepted; G0 exit is not.** The
implementation worktree is based on
`6c115aefe04ace0d169a24fa7cd55ad7c1befa52`.

The new contract crate adds distinct validated internal IDs, `Correlation`,
bounded `ExternalRef`, actor and target references, and a Runtime binding that
includes its instance and generation. Gateway storage now persists a Task
owner, typed event identities, and actor-scoped idempotency receipts, while the
Broker validates complete authoritative Actor provenance. It does not add an
actor registry, durable external-reference mapping, full Runtime/capability
relations, or an active Runtime-generation admission fence.

## Evidence reviewed

| Source/symbol | Verified fact |
| --- | --- |
| [`ProviderSessionId::parse`](../../../../../crates/cosh-core/src/session.rs#L28) | Rejects non-canonical provider-session UUIDs before path construction |
| [`PersistedSession.workspace_scope`](../../../../../crates/cosh-core/src/session.rs#L83) | Provider history is bound to a canonical workspace |
| [`AuditIdentity`](../../../../../crates/cosh-shell/src/types/audit.rs#L29) | Existing audit correlation uses optional string fields |
| [`ShellCommandAuditIdentity`](../../../../../crates/cosh-shell/src/types/mod.rs#L55) | Shell handoff carries Run, request, and tool references separately |
| [`ProviderToolKey`](../../../../../crates/cosh-shell/src/runtime/provider_tool_state.rs#L236) | Tool state scopes tool ID by Run in memory |
| [`RunCommand`](../../../../../crates/cosh-shell/src/adapter/cosh_core_service/command.rs#L21) | Core service carries Run and session scope but no durable binding generation |
| [`ids.rs`](../../../../../crates/cosh-gateway-contracts/src/ids.rs) | Sixteen prefixed internal ID newtypes share canonical generation, parsing, serde, and cross-type rejection |
| [`common.rs`](../../../../../crates/cosh-gateway-contracts/src/common.rs) | `Correlation`, `ActorRef`, `RuntimeBindingRef`, digests, and bounded values are typed |
| [`external.rs`](../../../../../crates/cosh-gateway-contracts/src/external.rs) | External namespace, authority, scope digest, and bounded opaque value are represented separately |
| [`task_store.rs`](../../../../../crates/cosh-gateway/src/storage/task_store.rs) | Task owner, events, projections, and actor-scoped key plus payload-digest receipts are committed transactionally; replay/conflict and actor-substitution tests exist |
| [`capability/broker.rs`](../../../../../crates/cosh-gateway/src/capability/broker.rs) | Request admission compares the complete authoritative `ActorRef`, including ID, issuer, kind, and assurance, with no Task-storage write |

Targeted tests exercised ID canonicalization, cross-type parsing, serde
validation, envelope schema matching, and size limits. No provider, ECS, or
host-mutation validation was needed for this side-effect-free crate.

## Acceptance matrix

| ID | Requirement | Baseline | Evidence required to pass |
| --- | --- | --- | --- |
| IC-01 | All internal lifecycle IDs are distinct validated newtypes | Pass for leaf types | Constructor, canonical serde, and cross-parse unit tests pass; property fixtures remain for G0 |
| IC-02 | Task, Run, Agent Session, Runtime binding, Approval, Permit, Execution, and Delivery parents are enforced | Partial | Runtime binding, capability request, and permit carry parents; database foreign-key and domain-constructor tests remain |
| IC-03 | Actor derives from authenticated issuer/subject, never request payload | Partial | Broker rejects complete Actor provenance substitution against its authoritative binding; authenticated ingress/IdentityResolver tests remain |
| IC-04 | Channel references include adapter, authority, conversation, and message scope | Partial | Kind, authority, scope digest, and opaque value are required; cross-tenant collision and retry fixtures remain |
| IC-05 | Provider and ACP IDs remain opaque external references | Partial | External kinds and bounded opaque values are typed; bridge tests with arbitrary, colliding, and non-UUID values remain |
| IC-06 | Runtime generation fences stale child output | Partial | Runtime binding carries instance and generation only; no active admission fence exists, and crash/restart delayed-event tests remain |
| IC-07 | Tool use and OS Execution identities are never conflated | Partial | Distinct `ToolUseId` and `ExecutionId` newtypes pass cross-parsing; multi-execution fixtures and durable constraints remain |
| IC-08 | Idempotency key reuse checks scoped payload digest | Partial | SQLite tests replay the same actor/key/digest and reject another digest or actor; authenticated ingress scope and channel fixtures remain |
| IC-09 | External identity values are bounded and redacted in diagnostics | Partial | Bounded construction and deserialization exist; injection, encryption/digest, and log tests remain |
| IC-10 | Legacy provider-session and Shell identities migrate without guessing Task identity | Design only | Dual-mode migration fixtures and explicit gap output |
| IC-11 | Audit schema change for new fields is reviewed explicitly | Missing | Accepted audit compatibility decision and reader tests |
| IC-12 | English/Chinese documents are equivalent and links resolve | Ready after doc validation | Recorded documentation checks |

## Required fixtures and artifacts

```text
fixtures/identity/v1/
  internal-ids.json
  correlation-complete.json
  external-channel-ref.json
  external-provider-session-ref.json
  external-acp-refs.json
  runtime-binding-generation.json
  approval-permit-execution-chain.json
  legacy-correlation-gap.json
  malformed/
    wrong-prefix.json
    noncanonical-id.json
    cross-tenant-message.json
    cross-task-run.json
    stale-runtime-generation.json
    oversized-external-value.json
    actor-substitution.json
```

Required implementation artifacts also include:

- an ID registry documenting prefix, scope, allocator, lifetime, and parent;
- database DDL with foreign keys and scoped unique indexes;
- a data-classification record for raw, encrypted, digested, and loggable
  external reference fields;
- audit compatibility fixtures for readers before and after the change;
- an exact mapping table for cosh-core, Shell, and ACP IDs.

The typed source exists; the listed versioned fixtures and durable artifacts
remain pending.

## Required validation commands

Final G0 acceptance must include these equivalent targeted commands:

```bash
cargo test --package cosh-gateway-contracts identity
cargo test --package cosh-gateway identity_resolver
cargo test --package cosh-gateway runtime_fencing
cargo test --package cosh-gateway --test identity_storage
cargo test --package cosh-shell --test protocol
```

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

Property-test seeds and test counts for storage, fencing, and ingress must be
retained when those remaining commands are added.

## Missing implementation

- Task owner/event/storage identity relations and actor-scoped receipts exist;
  full actor registry, external-reference, Runtime-binding, Approval, Permit,
  Execution, and Delivery relations or foreign keys remain absent.
- No actor mapping registry or authenticated identity resolver.
- No Runtime event-admission fence; only the generation-bearing binding type exists.
- No channel identity or scoped ingress idempotency.
- No durable ACP Connection, Session, Request, Message, Tool Call, or Terminal mapping;
  the corresponding external kinds exist only as pure references.
- No accepted audit evolution for new correlation fields.

## Exit criteria

G0 identity acceptance requires:

1. IC-01 through IC-12 pass on one recorded implementation commit.
2. Prefix and UUID representation are frozen by ADR.
3. All parent relations are enforced in constructors and storage.
4. Runtime restart tests prove stale output cannot mutate a Task.
5. Channel replay and actor-substitution tests fail closed.
6. ACP fixtures prove external values remain opaque and connection-scoped.
7. Logs, errors, and audit output contain no raw sensitive external identity.
8. Legacy records surface correlation gaps rather than invented identities.

## Validation recorded for this slice

- Reciprocal English/Chinese links are present.
- Tables, code blocks, ID names, and fixture lists are semantically aligned.
- Relative source links were checked from this directory.
- Markdown whitespace and diff hygiene were checked.
- Targeted formatting, package tests, Clippy, rustdoc, and dependency audit
  passed with the commands recorded above.
- ECS, provider, and host-mutation validation was intentionally skipped because
  this crate has no I/O or host behavior.
