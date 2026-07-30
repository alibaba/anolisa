# ActPlane Stale Violation Drain Design

## Problem

AgentSight reuses ActPlane maps and links pinned under `/sys/fs/bpf/actplane/v1`.
`ReloadHandle::clear_runtime_state()` clears policy counts and capability maps,
but it does not consume records already committed to the pinned kernel ring
buffer. After repeated test bindings, stale records can remain ahead of events
for a newly applied binding. The enforcer then spends its polling budget on
obsolete domain IDs, so the new denial is enforced in the kernel but its
violation is not delivered to AgentSight.

A controlled production experiment held binaries, configuration, database,
and verification procedure constant. Removing and reinstalling only the pinned
runtime changed delivery from neither subscriber receiving the violation to
both the direct UDS subscriber and AgentSight REST API receiving the same event.

## Selected Design

The enforcer prepares a reused singleton in this order:

1. Acquire the exclusive ActPlane runtime lock.
2. Protect the enforcer process.
3. Clear stale policy and capability runtime state.
4. Consume every event currently queued in the pinned ring buffer.
5. Create the in-memory binding registry and start the live violation poller.
6. Accept policy requests and report the backend ready.

Clearing the capability state before draining prevents old bindings from
producing a sustained stream while the finite ring buffer is consumed. The
drain discards all queued ActPlane event types because the singleton runtime is
exclusively owned and no prior binding metadata survives process restart.

## Alternatives Rejected

- Reinstall the complete pin tree on every startup: effective but destructive,
  slower, and incompatible with the intended pinned-singleton lifecycle.
- Ignore unmatched domain IDs in the live callback: already happens, but stale
  records still occupy the queue and delay current violations.
- Increase subscriber timeouts: masks the backlog and does not bound recovery.

## Error Handling

Failure to open or drain the ring buffer fails backend initialization. The
service must not report ready or accept a policy when it cannot prove that the
event stream starts from a clean boundary.

## Verification

- Unit tests prove startup preparation clears state before draining and stops
  before draining when cleanup fails.
- The reviewed ActPlane patch queue remains source-attested by blob hash.
- Linux checks run formatting, Clippy, unit/workspace tests, documentation, and
  release builds.
- The real-host closure test requires baseline read, enforced `EPERM`, identical
  UDS/API event ID, epoch timestamp, successful detach, and restored read.
