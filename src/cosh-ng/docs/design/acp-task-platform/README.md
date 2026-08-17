# ACP Task Platform Planning Set

[中文版](README_zh.md)

## Status

- Planning baseline: `e90d9d9402c7fa1c8122267eb4e075c0adda51f5`
- Candidate worktree: uncommitted implementation slices based on that baseline
- Document date: 2026-08-16
- Overall Phase 0-2 readiness: **NOT ACCEPTED**
- Scope: architecture, acceptance criteria, and first-slice implementation evidence

This set defines the first three delivery phases for evolving cosh-ng from an
interactive Agent shell into a local-first Agent OS gateway. ACP v1 is one
Agent Runtime adapter in this architecture. It is not the channel ingress,
durable task store, authorization system, or remote-control transport.

None of these capabilities is available on the pinned baseline. The candidate
worktree adds a runnable local Gateway and a contained, task-only production
profile, but it is not a universal production Gateway and has no distinct
candidate commit SHA yet. The profile exposes only `ask_user_question`; this
PR wires no production `ExecutionTarget` and has no checkpoint or ws-ckpt
dependency.

## Candidate implementation snapshot

The current worktree contains these partial foundations:

- [`cosh-gateway-contracts`](../../../crates/cosh-gateway-contracts/src/lib.rs):
  side-effect-free, versioned Gateway/Task schema v1, independent Runtime
  contract schema v4, Capability contracts, bounded leaf strings/digests, and
  distinct internal/external identities;
- [`cosh-gateway` Task and storage](../../../crates/cosh-gateway/src/task.rs): a
  pure Task reducer plus a local single-writer SQLite WAL store that commits
  events, projections, idempotency receipts, and Outbox intents together;
- [`RuntimeSupervisor`](../../../crates/cosh-gateway/src/runtime.rs): direct child
  launch validation, bounded stdout/stderr, process-group escalation/reap, and
  one process terminal observation;
- a strict codec for **private COSH JSONL control protocol v1**, including
  exact initialization and typed runtime-local observations. It is not ACP;
- an initial [`AcpV1RuntimeBridge`](../../../crates/cosh-gateway/src/runtime/acp.rs)
  that uses official Rust SDK 2.0.0 types for ACP wire v1, retains
  `RuntimeSupervisor` as the sole process-lifecycle implementation, and covers initialization, one
  session, text prompts, updates, permission correlation, and cancellation.
- a built-in [`ACP runtime profile resolver`](../../../crates/cosh-gateway/src/runtime/profile.rs)
  for installed `codex-acp` and `claude-agent-acp` executables. Descriptors pin
  the exact executable inode and workspace directory, the workspace digest
  binds canonical path, device, and inode, and an environment allowlist leaves
  no shell/package-runner/network bootstrap path.

Capability contracts, a durable schema-v9 Task/Run/Outbox/lease/input ledger
slice, installed ACP entrypoint, durable provider-native approval, neutral
Core/ACP Runtime ports, and a local Unix Gateway daemon/client slice now exist.
The local control slice supports peer-authenticated Task
submit/get/events/cancel/retry/append-input/resolve-approval. Production `serve`
admits only the contained brokered Core task-only profile, whose immutable
inventory contains `ask_user_question` and no production `ExecutionTarget`.
ACP remains an explicitly ungoverned `doctor`/`run` interoperability path.
The scheduler uses fenced Outbox leases, persists Runtime binding before
prompting, and fails closed after restart when a process cannot be reconnected.
Generic Capability/Permit/Execution contracts and ledger rows remain reusable
future foundations; they are not evidence of a production execution loop.
Checkpoint and ws-ckpt integration is a follow-up optional capability and is not
implemented or required by this PR. This is not coverage for Shell, Skill, MCP,
extension tools, or a universal Broker. The worktree still has no remote/network
API, Shell attachment, Web UI/API, DingTalk/Feishu adapter, or complete closure
of legacy execution paths. Existing `cosh-shell` continues to own its PTY and
compatibility cosh-core process path.

Contract and Runtime reducers now have aggregate admission, sequence, byte, and
transition matrices. The Task reducer covers 21 event kinds across 9 states.
The raw Task writer is crate-private in release builds; every single Task or
Outbox payload is capped at 256 KiB and each complete commit at 1 MiB. Runtime
input requests are durable, but the raw response remains only in a private
dispatch row while Task events and receipts retain its digest. A broader
collection/envelope compatibility corpus remains part of the complete gate.

## Product decision

COSH should own the durable task and OS-governance boundary while allowing
Shell, Web, DingTalk, Feishu, and automation clients to attach through stable
ports. This differs from treating Terminal UI, provider processes, or ACP
sessions as the product's source of truth.

ACP integration uses:

- ACP wire protocol v1 with `initialize.protocolVersion = 1`;
- official Rust SDK 2.0.0 pinned exactly in `Cargo.lock`, with the cosh-ng
  workspace and RPM build baseline raised to Rust 1.88;
- capability negotiation for every optional method or payload;
- local stdio transport in Phase 2;
- COSH-owned Gateway APIs for Web, channel, and cross-device traffic.

ACP v2 and the draft Streamable HTTP transport are outside the Phase 0-2
delivery contract.

The installed ACP slice has built-in launch profiles, durable Runtime binding,
Task event mapping, restart convergence, and independent cancellation. Its
signed/offline distribution, real Codex/Claude, and manual Terminal paths are
still not accepted. ACP filesystem/terminal callbacks and universal Shell,
Skill, MCP, or extension tool governance remain outside the accepted slice. The narrower
[local ACP MVP](task-plane/acp-mvp/design.md) is specified separately from the
full Phase 2 bridge.

## Reading order

1. [Cross-phase architecture](architecture.md)
2. [Warp comparison and positioning](warp-comparison.md)
3. Phase 0 module designs and readiness reports
4. Phase 1 module designs and readiness reports
5. Phase 2 module designs and readiness reports
6. [Overall acceptance report](acceptance-report.md)

## Module inventory

Every module has a design document and an acceptance report in English and
Chinese. Reports distinguish the pinned upstream baseline from partial
candidate-worktree evidence; neither document completeness nor a library slice
implies phase acceptance.

| Phase | Module | Design | Acceptance | Target delivery result |
| --- | --- | --- | --- | --- |
| 0 | Protocol contracts | [Design](foundations/protocol-contracts/design.md) | [Report](foundations/protocol-contracts/acceptance.md) | Versioned domain and port contracts |
| 0 | Identity and correlation | [Design](foundations/identity-correlation/design.md) | [Report](foundations/identity-correlation/acceptance.md) | Non-ambiguous actor and lifecycle identity |
| 0 | Storage and supervision | [Design](foundations/storage-supervision/design.md) | [Report](foundations/storage-supervision/acceptance.md) | Accepted persistence and process-owner ADRs |
| 1 | Gateway API | [Design](task-plane/gateway-api/design.md) | [Report](task-plane/gateway-api/acceptance.md) | Local admission and task command surface |
| 1 | Task Execution Plane | [Design](task-plane/task-execution-plane/design.md) | [Report](task-plane/task-execution-plane/acceptance.md) | Durable Task, event, lease, and Outbox state |
| 1 | Capability Broker | [Design](task-plane/capability-broker/design.md) | [Report](task-plane/capability-broker/acceptance.md) | One governed boundary for OS side effects |
| 1 | CoshCore Bridge | [Design](task-plane/cosh-core-bridge/design.md) | [Report](task-plane/cosh-core-bridge/acceptance.md) | Existing JSONL runtime behind a neutral port |
| 1 | Local ACP Runtime MVP | [Design](task-plane/acp-mvp/design.md) | [Report](task-plane/acp-mvp/acceptance.md) | One installed local stdio text-prompt path |
| 2 | ACP Client Bridge | [Design](adapters-and-presentation/acp-client-bridge/design.md) | [Report](adapters-and-presentation/acp-client-bridge/acceptance.md) | ACP v1 stdio Agent interoperability |
| 2 | Shell Attachment | [Design](adapters-and-presentation/shell-attachment/design.md) | [Report](adapters-and-presentation/shell-attachment/acceptance.md) | Shell attach/detach without losing PTY ownership |
| 2 | Web and Presentation | [Design](adapters-and-presentation/web-presentation/design.md) | [Report](adapters-and-presentation/web-presentation/acceptance.md) | Replayable Web/API views and reliable delivery |

## Phase gates

| Gate | Must be true before exit | Must not be deferred |
| --- | --- | --- |
| G0 Contract freeze | Schemas, ID invariants, capability vocabulary, persistence ADR, supervision ADR, fixtures, and compatibility policy are reviewed | Runtime-specific objects do not leak into Gateway or Task contracts |
| G1 Local durable gateway | Task state survives restart; command/event/outbox transaction rules hold; every OS write requires a target-bound permit; cosh-core is reachable through the Runtime Port | No API handler, presenter, or Agent bridge can write Task state or execute OS actions directly |
| GM Local ACP Runtime MVP | One installed local entrypoint runs one canonical workspace/session/active text prompt through `codex-acp` or `claude-agent-acp`; independent cancel, once-only permission decisions, fail-closed transport, and real-adapter conformance pass | No native Codex/Claude ACP assumption, package-runner/network bootstrap, filesystem/terminal capability, load/resume, Web/daemon dependency, or persistent permission rule |
| G2 ACP and attachments | ACP v1 conformance passes over stdio; permission and terminal requests enter COSH governance; Shell and Web can attach, detach, replay, approve, and cancel against the same Task | ACP is not used as a remote channel protocol, and ACP Session ID is never used as Task ID |

## Change-control rules

- A phase cannot redefine an earlier frozen identifier or event without a
  compatibility decision and updated fixtures.
- Each implementation pull request must cite the module acceptance rows it
  satisfies and attach the exact commands and evidence.
- Acceptance evidence must record the tested commit. A design review alone
  cannot mark runtime behavior as passed.
- An exact candidate, real Codex/Claude conformance, manual Terminal validation,
  signed artifacts, and power-loss evidence remain unaccepted external gates.
  ECS and exploratory dirty-worktree observations do not close them.

## External references

- [ACP architecture](https://agentclientprotocol.com/get-started/architecture)
- [ACP v1 initialization](https://agentclientprotocol.com/protocol/v1/initialization)
- [ACP v1 transports](https://agentclientprotocol.com/protocol/v1/transports)
- [ACP updates](https://agentclientprotocol.com/updates)
- [Warp Oz Platform](https://docs.warp.dev/platform/overview/)
- [Warp architecture and deployment](https://docs.warp.dev/enterprise/enterprise-features/architecture-and-deployment)
