# Provider-owned Suspension

[中文版](provider-suspension_zh.md)

## Scope

Blaze can hibernate a provider-backed sandbox without retaining its active
data-plane lease. The optional `DataPlaneSuspend` extension pairs
daemon-owned backend snapshot artifacts with immutable data-plane content that
remains owned by the build-time provider. Resume always prepares a fresh,
exclusive lease; it never reactivates the lease that was released during
hibernation.

This contract is used only when the build-time provider exposes the suspension
extension. A provider that declares `daemon_managed_storage`, including the
standard file provider, keeps the existing storage hibernation path without
this extension. The durable lifecycle state stores a portable provider
reference rather than depending on an extension's resource model. That
ownership record is not part of the management HTTP response.

## Ownership model

One provider-backed hibernation image has three distinct owners:

| Owner | Durable responsibility |
|---|---|
| Blaze | Sandbox lifecycle, backend snapshot payload, artifact hashes, operation phase, and public hibernation directory |
| Provider | Immutable root-filesystem and guest-memory content, content manifest, and all provider cleanup state |
| Shared contract | Provider instance, suspension, opaque reference and source-lease identities, logical extents, and manifest digest |

The suspension identity is allocated before provider mutation and makes an
unknown capture retryable. The opaque reference identifies the exact immutable
object returned by the provider. Blaze records both identities because a
request can have an unknown outcome before an exact reference is received.

The reference has no per-resource ownership flags. Its presence means one
provider owns both the writable-root and guest-memory views as a complete,
internally consistent image. Partial ownership is outside this contract because
Blaze does not coordinate an atomic capture across multiple data-plane owners.

## Guest recovery contract

Provider-backed hibernation requires a guest transport and protocol version 1.
Before any backend or provider mutation, Blaze uses `hello` to require these
operations:

- `prepare_hibernate`, which confirms that guest writes have been synchronized;
- `reseed_rng`, which accepts exactly 256 bytes of fresh host entropy after
  restore; and
- `post_restore`, which synchronizes the guest real-time clock and returns
  evidence of the resulting timestamp.

Missing operations fail the hibernation request without changing the sandbox
or provider. A failed or ambiguous mutating guest response is not treated as a
successful hook. Blaze does not publish the restored backend as running until
entropy injection and clock synchronization have both succeeded.

These hooks are guest behavior, not provider capabilities. A provider cannot
advertise or emulate them on behalf of an incompatible guest image.

## Hibernation transaction

The successful sequence is:

1. validate the active finalized lease, backend identity, restore adapter, and
   guest operations;
2. ask the guest to synchronize writes;
3. persist the suspension identity and the `Hibernating` operation;
4. quiesce the backend and capture its full snapshot while writes remain
   stopped;
5. ask the provider to capture immutable data-plane content at that same
   quiescent boundary;
6. validate and persist the provider reference and advanced active lease;
7. hash and synchronize the daemon-owned backend payload and manifest;
8. terminate the backend, stop and release the active provider lease, and
   persist that the active data plane is absent;
9. atomically publish the hibernation directory and transition to
   `Hibernated`; and
10. retire any older suspension only after the replacement image is durable.

If capture returns `OutcomeUnknown`, Blaze does not repeat the capture blindly.
It inspects the active lease and accepts only the exact generation before the
call or its exact one-generation successor, then asks the provider to retire
content by the preallocated suspension identity. A known reference is retired
by its complete public and provider identity. Before backend termination,
successful cleanup is followed by backend unquiesce and a return to `Running`.
After backend termination or whenever inspection or cleanup cannot be proven,
Blaze retains the write-ahead ownership ledger and enters `RecoveryRequired`.

## Resume transaction

Resume first verifies the public manifest, every backend artifact hash, and the
provider reference. It then:

1. allocates a new request, operation, and lease identity;
2. asks the provider to prepare a fresh lease from immutable suspension
   content;
3. validates either a path-backed slot or typed opened root-drive and
   guest-memory attachments and records the replacement lease;
4. starts the backend from the verified daemon payload and provider resources;
5. verifies backend identity, artifact identity, liveness, and guest protocol;
6. injects fresh entropy, clears the host copy, and synchronizes guest time;
7. commits the replacement lease and records the provider commit boundary;
8. publishes `Running`, retains the active lease, and finalizes provider
   ownership.

A failure before `Running` terminates the replacement backend, aborts the
replacement lease, and returns to the unchanged `Hibernated` image when cleanup
is proven. A failure after public publication cannot be rolled back safely and
therefore enters `RecoveryRequired`.

## Deletion and retirement

The hibernation image remains immutable and reachable after a successful
resume. It is retired only when a later hibernation replaces it or sandbox
deletion removes it. Blaze first persists a pending retirement record, then
removes the public owner, and only then requests provider retirement. Known
references and unknown captures are retired through separate request shapes so
the manager never invents an opaque provider identifier.

Deletion also releases any active or replacement lease before it commits the
terminal `Destroyed` state. A terminal record is invalid while it retains an
active lease, replacement lease, suspension reference, or pending suspension
retirement.

## Provider requirements

An implementation of `DataPlaneSuspend` must:

- make `suspend` idempotent by the preselected suspension identity;
- capture a root-and-memory view from the quiescent source lease and advance
  that finalized lease by exactly one generation;
- keep suspension content immutable after the source lease is released;
- make `resume` idempotent by the fresh `RequestContext` and return a distinct
  prepared lease with exact logical extents;
- return either one valid path-backed slot or one complete set of typed opened
  attachments;
- make retirement idempotent by the complete provider instance, public
  suspension, and optional reference identity; accept the same deterministically
  derived operation identity after a daemon restart; and
- retain enough durable state for inventory and operator recovery after a
  daemon interruption.

The suspension and checkpoint extensions are independent. A provider may
implement either extension without implementing the other.

## Verification boundary

The public tests cover contract validation, guest negotiation, entropy and
clock response validation, active-lease release, fresh-lease resume, daemon
restart while cleanly hibernated, startup retry of obsolete retirement records,
pre-mutation rejection of an incompatible guest, retry after backend-start
failure, and retirement during deletion.

The current manager fails closed for an interruption inside an unfinished
hibernate or resume transaction: it preserves the ledger and requires recovery
or explicit deletion instead of guessing whether a side effect occurred. A
production provider still needs Linux integration evidence for real backend
and guest behavior, repeated hibernate/resume cycles, process termination at
each persisted phase, uncertain provider and guest responses, concurrent
operations, content integrity, and zero residual resources after deletion.
