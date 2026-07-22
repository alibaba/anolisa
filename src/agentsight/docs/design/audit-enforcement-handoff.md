# Audit-to-Enforcement Handoff

## Goal

Upgrade an eligible credential-exfiltration audit case to temporary kernel enforcement while the
pinned ActPlane runtime owns one binding at a time. The handoff must remain recoverable across
AgentSight or enforcer restarts, preserve the original audit provenance, and restore audit coverage
when containment expires or cannot be installed.

## Constraints

- ActPlane accepts one active binding in the current pinned compatibility profile.
- A containment action is durable intent; an `enforced` binding is only an ActPlane acknowledgement;
  `blocked=true` remains the only proof that the kernel denied an operation.
- The original audit binding and its immutable evidence remain the source of policy provenance.
- AgentSight and the enforcer use separate SQLite and UDS boundaries, so no database transaction can
  be atomic with a kernel policy transition.
- Arbitrary ActPlane DSL and raw backend failures must not cross the product API boundary.

## Considered Approaches

### Durable compensated handoff in AgentSight

AgentSight records the containment action before changing enforcement state, serializes the source
detach and containment apply through the existing enforcement lifecycle lock, and compensates by
restoring audit when a terminal apply failure occurs. Restart reconciliation treats the durable
containment action as the transaction log.

This has the smallest interface change, but two independent UDS calls leave a race window where
reconnect reconciliation can interleave. A crash after detach or after kernel apply but before the
acknowledgement is persisted also leaves two binding rows that the current UUID-ordered replay
cannot resolve deterministically.

### Serialized replace command with durable transition intent

A new UDS command compiles the replacement, detaches the source, and attaches the replacement under
the backend lifecycle lock. AgentSight persists a transition intent before the call and reconciles
that transition before ordinary desired bindings after restart.

This is the selected approach. It expands the protocol, client, service, backend, and persistence
surface, but it is the only option that both encapsulates the singleton constraint in its owner and
makes crash recovery deterministic.

### Multiple simultaneous ActPlane bindings

Keeping audit and enforcement active together would avoid a handoff, but conflicts with the pinned
singleton profile and its global runtime cleanup assumptions. This would require a larger upstream
ActPlane lifecycle redesign and is outside the containment scope.

## Selected Design

### Durable transition identity

Each new containment action persists the source audit binding ID in addition to the containment
binding ID. The source ID is immutable after the action is claimed. Existing rows without this field
remain readable and fail closed during recovery if provenance cannot be reconstructed exactly.

The enforcement store retains the complete source `ApplyPolicy` after its state becomes `detached`.
That request is the only policy accepted for audit restoration; AgentSight never rebuilds audit DSL
from redacted UI data.

The enforcement store also owns a transition record keyed by containment action ID and direction.
It contains the complete source binding snapshot, a typed replacement request, lifecycle phase,
optional acknowledgement, bounded failure detail, and update time. Forward and reverse transitions
therefore have distinct, stable keys. The record is created before the privileged call and is the
only authority for deciding which binding must own the singleton after restart.

`security.db` and `enforcement.db` remain separate databases. The design does not claim a cross-file
transaction: `security.db` first claims the pending action, then `enforcement.db` atomically writes
the transition while retaining the source desired state. The stable action ID makes transition
creation idempotent after a crash between those steps.

### Replace protocol

The versioned UDS protocol adds a semantic replace operation:

```text
replace(expected_binding, replacement)
```

`replacement` is a typed enum containing either a generic `ApplyPolicy` or a product-level
`ApplyCredentialPolicy`. This lets the forward handoff compile the credential policy inside the
privileged adapter and lets the reverse handoff restore the exact original audit request. The
command requires the complete expected source binding snapshot. Its response distinguishes:

- replacement acknowledged;
- source retained or restored after replacement rejection;
- indeterminate runtime state when both replacement and restoration fail.

Unknown or mismatched actual bindings are conflicts and are never detached. Request and response
frames remain bounded and correlated. The protocol version is advanced because older peers do not
understand the new lifecycle guarantee.

### ActPlane replace operation

The ActPlane backend holds its existing lifecycle mutex for the entire replacement:

1. Validate the expected identity, replacement process identity, and product policy.
2. Compile the replacement before changing kernel state.
3. Inspect actual state: replacement already active is idempotent success; expected source active
   proceeds; an empty runtime installs the transition's durable target during recovery; any third
   binding is a conflict.
4. Detach the expected source and install the compiled replacement without releasing the lock.
5. If installation fails, immediately restore the exact source request.
6. Return the replacement acknowledgement, a source-restored rejection, or an indeterminate result
   if restoration also fails.

Compilation and process validation failures occur before detachment and therefore have no runtime
side effect. Existing standalone apply and detach operations use the same lifecycle mutex and cannot
interleave with replacement.

### AgentSight transition coordinator

The containment action is claimed first. The coordinator then writes the forward transition intent
(`audit -> enforce`) in `enforcement.db` before crossing UDS. The source row remains present but is
excluded from ordinary replay while its transition is live; the product-level target request lives
in the transition because its compiled `ApplyPolicy` does not exist until the privileged adapter
acknowledges it. A successful response atomically persists source `detached`, inserts target
`enforced`, and marks the transition `completed` before the containment action can become active.

A source-retained or source-restored rejection marks the transition failed and may finish the
containment action as failed because audit coverage is assured. An indeterminate or transport result
keeps the transition and containment action retryable; it must not claim either policy is active.

The normal desired-state reconciler always processes unfinished transitions before individual
bindings. Source and target rows participating in a transition are excluded from UUID-ordered replay
until the transition reaches a terminal state.

### Restart convergence

The persistent containment action is written before source detachment, which makes the relevant
crash windows converge:

| Crash window | Reconciliation result |
| --- | --- |
| After intent persistence, before replace | Actual source causes replace to run; empty runtime installs the durable target. |
| During backend replace | The backend lock excludes other lifecycle operations; failure restores the source or reports indeterminate. |
| After backend replace, before acknowledgement persistence | Actual target completes the transition idempotently. |
| After acknowledgement persistence, before action activation | Reuse the exact containment binding and activate the action. |
| During failed replacement restoration | Actual source marks the upgrade failed with audit retained; empty or unknown state remains retryable and degraded. |
| During reverse expiry replace | Actual audit completes expiry; actual containment retries the reverse replace. |

Reconciliation queries actual backend bindings before deciding a transition outcome. Target actual
means complete, source actual means execute or record a restored failure as appropriate, empty actual
installs the durable target, and a third binding is a visible conflict that is never destroyed.

Initial plan and containment creation still require an enforced audit binding. After an action is
claimed, recovery uses only the complete source request snapshot frozen in its enforcement
transition, and verifies its binding, Agent, session, policy, revision, process, and evidence identity.
Detached state alone never makes an unrelated binding eligible.

### Expiry

Temporary containment expiry creates a reverse transition (`enforce -> audit`) using the same replace
protocol:

1. Persist the action as expiring.
2. Persist the reverse transition intent and exact original audit request.
3. Replace the containment binding with the audit binding and require acknowledgement.
4. Atomically persist containment `detached`, audit `enforced`, and the reverse transition completed.
5. Persist the action as expired only after the audit acknowledgement is durable.

This restores observation after the blocking window. A retryable or indeterminate replacement keeps
the action expiring and is retried with bounded backoff.

## Error Handling

- Binding identity, request, process start time, policy ID, revision, and source path must match at
  every transition.
- Raw UDS errors, filesystem paths, policy DSL, and kernel details are converted to bounded,
  allowlisted operator messages before persistence or API exposure.
- Duplicate requests return the existing action when target and duration match; conflicting repeats
  are rejected.
- Lost lifecycle claims and readiness generation changes abort activation and are reconciled instead
  of being treated as success.

## Verification

Automated coverage must include:

- successful audit detach, containment apply, block observation, expiry, and audit restoration;
- apply rejection with successful audit compensation;
- compensation failure remaining retryable;
- every restart window in the convergence table;
- duplicate requests, stale PID reuse, source-binding mismatch, and readiness generation changes;
- exact persistence of source/containment IDs and sanitized failure messages;
- protocol serialization, version rejection, request correlation, and bounded replace responses;
- backend replacement idempotency for source, target, empty, and third-party actual states;
- replacement compilation and PID validation having no pre-detach side effects;
- replacement attach failure with both successful and failed source restoration;
- transition-store compare-and-swap behavior and restart convergence at every tabled crash point;
- concurrent apply, detach, and replace operations remaining serialized;
- existing standalone enforcement APIs and desired-state reconciliation remaining compatible.

Native Linux acceptance uses Hermes to produce:

```text
audit file read -> taint -> network decision (allowed)
upgrade to containment
file read -> taint -> network decision (blocked=true, EPERM)
wait for expiry
file read -> taint -> network decision (allowed and audited)
```

The report records binding IDs, lifecycle states, timestamps, and redacted destinations, but never
the sensitive file content, credentials, Dashboard token, or policy DSL.
