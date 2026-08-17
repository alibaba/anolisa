# Durable Task Plane

[中文版](durable-task-plane_zh.md)

Related architecture: [COSH Gateway and ACP Architecture](README.md)

## Purpose

The Task Plane is the durable coordinator for Task state, scheduling intent,
Runtime ownership, and delivery. It is the only source of truth for lifecycle
decisions; an in-memory worker or provider process is not authoritative.

## Durable records

The persistence model separates:

- append-only Task events;
- current Task projection and revision;
- idempotency receipts;
- Outbox delivery intents;
- Run lease and Runtime binding;
- Approval, input, execution, and Runtime-dispatch ledgers;
- bounded audit and reconciliation evidence.

Each record carries exact internal identities. Raw provider output and secrets
are not Task history.

The admitted Runtime profile and Gateway capability profile are immutable Run
inputs. Scheduling and retry reconstruct their exact identities from durable
state; they do not re-resolve a profile from current host availability. A
profile change creates a new explicitly admitted Run or fails closed.

## Atomic command boundary

A Task command commits, in one transaction:

1. expected-revision and idempotency validation;
2. accepted Task events;
3. the reduced projection;
4. the durable command receipt;
5. Outbox intents required by the transition.

Either every item commits or none does. The writer validates event count,
Outbox count, individual payload size, and aggregate command size before the
transaction mutates storage.

## Outbox

Outbox is the durable boundary between state transition and external delivery.

- A stable Delivery ID identifies one logical send.
- Claim and acknowledgement are separate durable transitions.
- A lost response reuses the same delivery identity.
- Malformed or permanently rejected entries move to bounded dead-letter state;
  they do not crash or busy-loop the daemon.
- Delivery does not grant execution authority. Authority remains in Approval,
  Permit, and Execution records.

## Run lease and Runtime binding

A worker must hold a current lease before starting or polling a Runtime. The
lease has an owner, generation, revision, and expiry. Renewal changes revision;
takeover changes generation and fences the old worker.

Before the first prompt, the worker persists a RuntimeBinding tied to the
current lease generation. Every callback and authority transition validates
the current Run, binding, and fence.

## Restart and takeover

On restart, Gateway classifies durable state instead of guessing what an old
process did:

- queued work with a valid Outbox intent can be claimed;
- work without reconstructable admission settles fail closed;
- an unacknowledged Runtime start becomes a known failure or uncertain state
  according to the durable boundary crossed;
- pending input and approval ledgers settle before Task terminalization;
- Started execution or delivery becomes `Uncertain`/`Unknown`, never automatic
  replay;
- typed target reconciliation may turn an uncertain result into a conclusive
  result when the target proves the exact operation outcome.

Recovery does not substitute one capability provider for another. If a Run
references a target that is no longer admitted, its unstarted work settles
fail closed and a Started effect remains uncertain until the same typed target
or an explicit operator procedure can reconcile it.

Explicit release is distinguished from lease expiry so a partially completed
takeover can resume without repeatedly reclaiming a settled suspended Run.

## Cancellation, retry, and input

- Cancellation is a durable request before process signaling.
- A safely quiescent suspended Run can be abandoned atomically; an uncertain
  effect cannot be silently cancelled as if it never happened.
- Retry creates a new Run only after the old lease, binding, input, approval,
  and delivery state are quiescent.
- Input append binds exact Task, Run, Runtime request, revision, and digest.
- Shutdown settles pending input and approvals before closing the binding.

## Storage and recovery contract

- SQLite uses WAL and FULL durability for the local single-writer profile.
- Schema migrations are checksummed and reject newer or divergent history
  without mutation.
- Startup validates integrity and foreign keys before accepting work.
- Backup is online, source-bound, no-clobber, and verified before publication.
- Restore targets a new path and verifies installation identity and schema.
- Operator inspect is read-only and returns bounded redacted health.

Filesystem authority must be held across validation and open; pathname checks
alone do not protect against rename or symlink races.

## Failure semantics

| Boundary | Recovery classification |
| --- | --- |
| Before Task transaction commit | No accepted command |
| After commit, before Outbox send | Reclaim stable delivery |
| Receiver may have accepted, no durable ack | Delivery `Unknown`; do not resend blindly |
| Permit claimed, no start audit | Known no effect only when audit gate proves it |
| Execution Started, no conclusive result | Execution `Uncertain`; reconcile or suspend |
| Result committed, API response lost | Replay durable result and receipt |

## Acceptance invariants

- Event, projection, receipt, and Outbox never diverge after a crash.
- A stale lease owner cannot write Task, Runtime, approval, input, or execution
  state.
- Response-loss replay does not duplicate an effect or Runtime delivery.
- Poison data cannot starve unrelated work.
- Every uncertain effect remains visible until exact reconciliation or operator
  settlement.
- Restart and retry preserve the admitted capability profile; provider
  unavailability cannot silently change a Run's operation inventory.
