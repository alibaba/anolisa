# ACP v1 Phase 0-2 Planning Set

[中文版](README_zh.md)

## Status

- Planning baseline: `up/main` at `e3763b001c91f3c13dc6afbd57aac924162e9f59`
- Candidate worktree: uncommitted implementation slices based on that baseline
- Document date: 2026-08-13
- Overall Phase 0-2 readiness: **NOT ACCEPTED**
- Scope: architecture, acceptance criteria, and first-slice implementation evidence

This set defines the first three delivery phases for evolving cosh-ng from an
interactive Agent shell into a local-first Agent OS gateway. ACP v1 is one
Agent Runtime adapter in this architecture. It is not the channel ingress,
durable task store, authorization system, or remote-control transport.

None of these capabilities is available on the pinned `up/main` baseline. The
candidate worktree adds library-level foundations only; it is not a production
Gateway and has no distinct candidate commit SHA yet.

## Candidate implementation snapshot

The current worktree contains these partial foundations:

- [`cosh-gateway-contracts`](../../../crates/cosh-gateway-contracts/src/lib.rs):
  side-effect-free, versioned Task/Runtime/Capability contracts, bounded
  leaf strings/digests, and distinct internal/external identities;
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
  for installed `codex-acp` and `claude-agent-acp` executables, with canonical
  workspace/executable validation, an environment allowlist, and no
  shell/package-runner/network bootstrap path.

Capability contracts, a durable ledger slice, installed ACP entrypoint,
once-only permission evidence, neutral Core/ACP Runtime ports, and a local
Unix Gateway daemon/client slice now exist. The local control slice supports
peer-authenticated Task submit/get/events/cancel, but does not schedule a
Runtime or consume Outbox work. The worktree still has no remote/network API,
complete ACP-to-domain governance, Shell
attachment, Web UI/API, DingTalk/Feishu adapter, restart/lease
orchestration, or complete production bypass closure. Existing `cosh-shell`
continues to own its PTY and compatibility cosh-core process path.

The contract foundation does not yet apply aggregate admission limits to all
collections and envelopes, including vectors, batches, and Outbox payloads.

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

The ACP slice is a library-level interoperability probe with built-in launch
profiles, not an installed production entrypoint. Filesystem/terminal
callbacks, durable `AgentSessionId` binding, Task event mapping,
restart/resume, independent cancellation, and real-adapter conformance remain
outside the implemented slice. The narrower [local ACP MVP](phase-1/acp-mvp/design.md)
is specified separately from the full Phase 2 bridge.

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
| 0 | Protocol contracts | [Design](phase-0/protocol-contracts/design.md) | [Report](phase-0/protocol-contracts/acceptance.md) | Versioned domain and port contracts |
| 0 | Identity and correlation | [Design](phase-0/identity-correlation/design.md) | [Report](phase-0/identity-correlation/acceptance.md) | Non-ambiguous actor and lifecycle identity |
| 0 | Storage and supervision | [Design](phase-0/storage-supervision/design.md) | [Report](phase-0/storage-supervision/acceptance.md) | Accepted persistence and process-owner ADRs |
| 1 | Gateway API | [Design](phase-1/gateway-api/design.md) | [Report](phase-1/gateway-api/acceptance.md) | Local admission and task command surface |
| 1 | Task Execution Plane | [Design](phase-1/task-execution-plane/design.md) | [Report](phase-1/task-execution-plane/acceptance.md) | Durable Task, event, lease, and Outbox state |
| 1 | Capability Broker | [Design](phase-1/capability-broker/design.md) | [Report](phase-1/capability-broker/acceptance.md) | One governed boundary for OS side effects |
| 1 | CoshCore Bridge | [Design](phase-1/cosh-core-bridge/design.md) | [Report](phase-1/cosh-core-bridge/acceptance.md) | Existing JSONL runtime behind a neutral port |
| 1 | Local ACP Runtime MVP | [Design](phase-1/acp-mvp/design.md) | [Report](phase-1/acp-mvp/acceptance.md) | One installed local stdio text-prompt path |
| 2 | ACP Client Bridge | [Design](phase-2/acp-client-bridge/design.md) | [Report](phase-2/acp-client-bridge/acceptance.md) | ACP v1 stdio Agent interoperability |
| 2 | Shell Attachment | [Design](phase-2/shell-attachment/design.md) | [Report](phase-2/shell-attachment/acceptance.md) | Shell attach/detach without losing PTY ownership |
| 2 | Web and Presentation | [Design](phase-2/web-presentation/design.md) | [Report](phase-2/web-presentation/acceptance.md) | Replayable Web/API views and reliable delivery |

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
- Full provider, ECS, or manual Terminal validation remains a separately
  requested gate; the planning documents do not imply that it has run.

## External references

- [ACP architecture](https://agentclientprotocol.com/get-started/architecture)
- [ACP v1 initialization](https://agentclientprotocol.com/protocol/v1/initialization)
- [ACP v1 transports](https://agentclientprotocol.com/protocol/v1/transports)
- [ACP updates](https://agentclientprotocol.com/updates)
- [Warp Oz Platform](https://docs.warp.dev/platform/overview/)
- [Warp architecture and deployment](https://docs.warp.dev/enterprise/enterprise-features/architecture-and-deployment)
