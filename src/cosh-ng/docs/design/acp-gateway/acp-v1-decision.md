# ACP v1 Integration Decision

[中文版](acp-v1-decision_zh.md)

Related architecture: [COSH Gateway and ACP Architecture](README.md)

## Decision

cosh-ng acts as an **ACP Client** for external agents, using stable wire
protocol v1 over local stdio. Version-controlled adapters connect Codex and
Claude Code to the same `AgentRuntimePort`; other agents can implement that
contract without adding provider branches to Gateway or Shell.

Protocol and package versions are managed independently:

- wire compatibility is negotiated through `initialize.protocolVersion`, with
  stable baseline `1`;
- the initial official Rust SDK baseline is `agent-client-protocol = 2.0.0`;
- a crate major version never implies a wire version;
- ACP v2 and unstable features require separate compatibility and security
  review.

The official SDK 2.0.0 requires Rust 1.88.0 and keeps stable v1 behavior
separate from experimental v2 features.

## Why ACP

- Decouple COSH from private output formats of individual agent CLIs.
- Reuse standard Session, Prompt, streaming update, Cancel, and Permission
  Request semantics.
- Let Codex, Claude Code, and future agents share one Task, Approval, and audit
  model.
- Preserve an on-device path that does not depend on a remote control protocol.
- Concentrate provider compatibility in adapters and a conformance suite.

## Boundary

```text
COSH Task Plane
    -> AgentRuntimePort
        -> ACP v1 Client Bridge
            <-> ACP Agent Adapter
                -> Codex / Claude Code / other agents
```

ACP provides runtime interoperability. It does not own:

- channel messages, chat threads, or cross-device connectivity;
- Task persistence, scheduling, idempotency, leases, or crash recovery;
- user identity, tenancy, RBAC, or OS capabilities;
- checkpoint, rollback, ECS management, or delivery receipts;
- a general IPC protocol between COSH components.

ACP is therefore a Runtime replaceability boundary, not the COSH control
protocol.

ACP Runtime selection is also independent from Gateway capability profile
selection. An admitted ACP adapter does not imply that any `ExecutionTarget`
is present. A `task-only-v1` deployment exposes no governed side-effect tool;
`ws-ckpt-v1` exposes checkpoint only through a typed hosted-operation contract
that the Runtime explicitly acknowledges.

Provider-native ACP permission is not a fallback execution target. An adapter
without a typed hosted-result contract cannot turn `allow_once` into a COSH
Permit or satisfy a checkpoint request on behalf of `ws-ckpt`.

## Stable capability scope

| Capability | COSH handling |
| --- | --- |
| `initialize` | Negotiate v1 and record both implementations and capabilities |
| `session/new` | Create one explicit SessionBinding for a Run |
| `session/prompt` | Submit a bounded text task and receive one terminal result |
| Session Update | Map text, plan, tool, and status updates to Runtime events |
| Permission Request | Forward to Approval; reject when trusted correlation is absent |
| Cancel | Propagate Task cancellation and guarantee eventual process reaping |
| Error/Close | Produce deterministic Runtime terminal state with bounded diagnostics |

Filesystem and terminal client capabilities, Session load/resume/fork, remote
HTTP transport, MCP-over-ACP, and experimental v2 features are outside this
stable baseline. Each requires an explicit data boundary, threat model,
compatibility decision, and conformance evidence.

## Mapping invariants

- A Task can have multiple Runs; a Run has at most one active ACP Session.
- ACP updates are normalized to provider-neutral Runtime events before they
  reach a presentation layer.
- An ACP Tool Call is not execution authority. A real effect still requires
  Approval and a target-bound Permit.
- A Permission Request is correlated to exact Task, Run, Actor, Target,
  Operation, and canonical digest.
- Prompt completion, process exit, and transport close are separate facts;
  the Runtime reducer chooses one terminal outcome.
- Unknown messages, oversized payloads, stale callbacks, and unsupported
  capabilities are rejected or explicitly ignored, never downgraded to allow.

## Adapter strategy

- Adapter and upstream agent versions are recorded in an explicit matrix.
- Production profiles use absolute normalized paths, fixed basenames, and a
  controlled environment.
- Launch clears the environment and explicitly inherits only required locale,
  proxy, and authentication entry points. Dynamic loader and Node injection
  variables are rejected.
- Installation, upgrade, and provenance checks are separate from Gateway
  runtime; Gateway never downloads packages while starting a Task.
- Every adapter upgrade reruns fake and real conformance.

## Conformance

A supported adapter must:

1. Pin adapter and agent versions and complete v1 initialize, Session creation,
   and a text prompt.
2. Map text, plan, tool, permission, cancellation, and error events to stable
   Runtime events.
3. Pass real `allow_once` and `reject_once`; denial produces no effect.
4. Settle timeout, cancel, crash, malformed frame, and transport close exactly
   once and reap the process tree.
5. Correlate Task, Run, Session, Agent, Adapter, Approval, Execution, and error
   evidence without reusing identifiers.
6. Produce redacted, repeatable evidence.

## Compatibility and rollback

ACP Runtime is enabled by profile. A protocol or adapter regression disables
that profile while preserving Task and audit history. Rollback does not rewrite
historic events and does not silently move a failed Run to another agent.

Runtime and capability profiles roll back independently. Their admitted pair
must have an explicit conformance entry; Gateway never invents a new pair for
an existing Run.

## Adding an agent

Prefer an ACP adapter over a provider-specific branch in Gateway or Shell. A
contribution includes a version profile, installation provenance, capability
matrix, fake conformance, and at least one real-agent conformance run.
Provider-private events are normalized inside the adapter boundary.

## References

- [ACP versioning](https://github.com/agentclientprotocol/agent-client-protocol#versioning)
- [Official ACP Rust SDK](https://github.com/agentclientprotocol/rust-sdk)
- [Rust SDK 2.0.0 manifest](https://docs.rs/crate/agent-client-protocol/2.0.0/source/Cargo.toml)
