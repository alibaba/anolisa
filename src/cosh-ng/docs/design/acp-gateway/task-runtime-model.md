# Task and Runtime Model

[中文版](task-runtime-model_zh.md)

Related architecture: [COSH Gateway and ACP Architecture](README.md)

## Purpose

This document defines the durable identities and provider-neutral port between
Gateway and an Agent Runtime. It prevents process, provider, and channel state
from becoming a second Task state machine.

## Identity hierarchy

```text
Installation
  -> Actor
  -> Task
      -> Run
          -> RuntimeBinding
          -> Turn
          -> Agent SessionBinding
          -> Request / ToolUse / Approval / Execution
```

Each identifier has one meaning and one namespace:

| Identity | Authority and lifetime |
| --- | --- |
| `InstallationId` | One installed Gateway authority domain |
| `ActorId` | Authenticated caller principal within an installation |
| `TaskId` | Durable user-visible work unit |
| `RunId` | One attempt of a Task |
| `RuntimeBindingId` | Fenced binding between one Run and one Runtime instance |
| `TurnId` | One prompt-to-terminal exchange inside a Run |
| `SessionBinding` | Mapping to a provider or ACP Session identifier |
| `RequestId` | One callback or input request |
| `ApprovalId` | Durable decision record for one bounded request |
| `PermitId` | Single-use authority for one execution |
| `ExecutionId` | One attempted external effect |

IDs are never substituted across domains. External IDs are stored as bounded
references and are always paired with their internal binding.

## Task, Run, and Turn

- A Task survives client disconnect, Runtime exit, and daemon restart.
- A Task can have multiple Runs, but only one active Run.
- A Run owns one lease generation and zero or one active RuntimeBinding.
- A Turn belongs to one Run. Turn completion does not automatically terminate
  a multi-turn Run.
- A retry creates a new Run; it never reopens a terminal Run.

Task events are append-only facts. A pure reducer validates expected revision,
state transition, and identity correlation before updating a projection.

## AgentRuntimePort

Gateway talks to runtimes through versioned, provider-neutral commands and
events.

Commands cover:

- initialize/start;
- prompt or turn input;
- exact response to a pending input request;
- cancellation and shutdown;
- approval acknowledgement and typed brokered result delivery.

Events cover:

- bounded observations;
- tool-use lifecycle;
- provider-native permission request;
- brokered operation request;
- exact input request;
- one terminal Run or Turn outcome.

Provider-specific payloads are normalized at the bridge. Channel-specific
presentation is derived later from durable events.

Runtime initialization also declares a bounded hosted-operation inventory.
Gateway compares it with the selected capability profile before accepting Task
work. `task-only-v1` requires an empty inventory; `ws-ckpt-v1` requires the
exact versioned checkpoint request and typed-result capability. Missing,
additional, or downgraded operations fail admission rather than being hidden
by presentation logic.

The Runtime can request only an operation. It cannot select the concrete
`ExecutionTarget`, socket, service, credential, or fallback path. Those remain
trusted daemon configuration bound to the admitted profile.

## Runtime binding fence

A callback is accepted only when all of the following match durable state:

- Actor, Task, and Run;
- RuntimeBinding ID and generation;
- current Run lease generation;
- request identity and expected revision;
- monotonic event sequence.

Lease renewal may change a lease revision without changing its authority
generation. A new generation fences every callback from the previous Runtime.

## Terminal semantics

Prompt completion, process exit, transport close, cancellation, and execution
uncertainty are distinct facts. The Runtime adapter emits at most one terminal
observation, and the Task reducer determines the durable Task outcome.

Late observations and callbacks after cancellation or terminal settlement fail
closed. A response-loss replay returns durable state and does not write to the
Runtime a second time.

## Versioning and bounds

- Gateway/Task and Runtime schemas version independently.
- Every string, collection, frame, and aggregate has an explicit bound.
- Unknown versions and unsupported operations return stable typed errors.
- A breaking Runtime change increments Runtime schema without implicitly
  changing ACP wire or Task schema.
- Machine-readable fixtures cover encoding, decoding, unknown fields, limits,
  and backward/forward compatibility.

## Acceptance invariants

- Two provider sessions cannot collide in the same internal identity.
- A stale Runtime cannot mutate a replaced Run.
- Retry creates a new Run and a new fence.
- A late permission or input response is rejected without partial mutation.
- Exactly one terminal outcome is observable through the port.
- Core and ACP adapters pass the same neutral-port contract suite.
- Runtime hosted-operation inventory exactly matches the admitted capability
  profile before a Run starts.
