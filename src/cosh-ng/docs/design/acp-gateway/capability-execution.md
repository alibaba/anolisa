# Capability and Execution

[中文版](capability-execution_zh.md)

Related architecture: [COSH Gateway and ACP Architecture](README.md)

## Purpose

This design separates a Runtime's request, a user's approval, COSH execution
authority, and the external effect. No protocol callback or UI action is itself
a Permit.

## Authority model

```text
Runtime request
  -> CapabilityRequest
  -> Policy decision
  -> Approval, when required
  -> ExecutionPermit
  -> Execution claim
  -> durable pre-effect audit
  -> typed ExecutionTarget
  -> typed result or uncertainty
  -> Runtime acknowledgement/result delivery
```

The authoritative objects are:

| Object | Meaning |
| --- | --- |
| `CapabilityRequest` | Canonical requested operation, actor, target, scope, and digest |
| `PolicyDecision` | Versioned allow, deny, or require-approval result |
| `Approval` | Durable human decision bound to the request and expiry |
| `ExecutionPermit` | Single-use authority bound to exact request, target, policy, and fences |
| `Execution` | One claim/start/complete lifecycle for an external effect |
| `ExecutionTarget` | Trusted adapter that validates and performs one typed operation |

Provider-native permission remains observational evidence unless COSH takes
over the operation through this flow. An ACP `allow_once` response does not
prove a COSH Permit was consumed at the effect boundary.

## Canonical operation

Every governed operation has a versioned typed representation. Its canonical
digest covers:

- operation name and version;
- bounded input;
- Actor, Task, Run, and Request identities;
- target identity and scope;
- policy revision and expiry;
- Runtime binding and lease generation.

Presentation text is not authority. Runtime labels can assist a user but cannot
change operation digest, target, or policy meaning.

## Approval

- Approval is asynchronous and durable.
- Only an authenticated actor authorized for the Task can resolve it.
- Approve and deny race through one revisioned state machine.
- Expiry is a terminal denial source and never creates an allow dispatch.
- Idempotent replay returns the original durable receipt.
- Policy and target are re-evaluated before the first Permit is issued.
- A previously issued durable Permit can be recovered without inventing a new
  policy decision.

## Permit and fence

A Permit is:

- single-use;
- bound to one `ExecutionId` and typed operation;
- bound to target identity digest;
- bound to RuntimeBinding and Run lease generation;
- time-bounded and policy-revision-bound;
- consumed atomically when execution is claimed.

Lease renewal inside one generation does not invalidate authority. A takeover
generation fences every old Permit that has not been consumed safely.

## Audit before effect

Execution cannot begin until a security audit record for its exact start is
durable. The audit boundary records bounded references for request, policy,
approval, Permit, execution, target, and fences. Raw secrets and unbounded tool
input do not enter audit.

If audit persistence fails before the target is called, execution settles as
known no effect. If audit durability itself is indeterminate, target execution
does not begin.

## Execution lifecycle

```text
Planned -> Claimed -> Started -> Succeeded | Failed | Uncertain
```

- `Claimed` proves the Permit was consumed but the target has not started.
- `Started` proves the audit barrier passed and an effect may have occurred.
- `Succeeded` stores a typed durable result.
- `Failed` is conclusive only when the target proves its result.
- `Uncertain` means an effect may have occurred; it cannot be retried
  automatically.

Typed result and terminal Execution state commit atomically. Runtime delivery
uses a separate durable dispatch ledger so response loss cannot duplicate the
effect.

## Reconciliation

An `ExecutionTarget` may provide a query-only reconcile operation. It receives
the exact persisted operation, target identity, and execution reference.

- Exact match can settle `Succeeded` or conclusive `Failed`.
- Absence is conclusive only when the target can prove the effect did not
  occur.
- Changed identity, incomplete evidence, timeout, or query failure remains
  `Uncertain`.
- Reconciliation never repeats the mutation.

## Cancellation

Cancellation prevents unstarted authority from progressing and settles pending
approval or input. It cannot erase an Execution that reached `Started`.
Cancellation of an uncertain execution leaves a visible suspended Task until
reconciliation or explicit operator settlement.

## Acceptance invariants

- No execution without a consumed exact Permit and durable start audit.
- Denial, expiry, stale fence, and policy change produce zero target calls.
- A Permit can be claimed once across process restart.
- Started execution is never automatically retried.
- A fabricated Runtime result cannot disagree with durable Execution state.
- Delivery replay writes to Runtime at most once.
