# Provider-owned Checkpoints

[中文版](provider-checkpoints_zh.md)

## Scope

Blaze can pair daemon-owned backend snapshot artifacts with immutable data-plane
content owned by a build-time provider. This is an optional extension of
`DataPlaneProvider`: a provider-backed sandbox uses it only when the compiled
provider exposes `DataPlaneCheckpoint`. A provider that declares
`daemon_managed_storage`, including the standard file provider, continues to
use the existing storage checkpoint path without this extension.

The contract represents extension-owned content through bounded opaque
identities and an integrity digest. Each provider materializes and validates the
content behind those identifiers. This lets file-, device-, or service-backed
implementations share the same Blaze checkpoint catalog.

The ownership reference is part of the durable checkpoint manifest used for
recovery, not the management HTTP response. API clients continue to receive
the public checkpoint identity, lineage, backend, snapshot kind, and artifact
metadata without provider lease or content identifiers.

## Ownership split

One public checkpoint is complete only when both sides of this boundary agree:

| Owner | Durable content |
|---|---|
| Blaze | Public checkpoint UUID, sandbox and runtime identity, parent, backend payload and hashes |
| Provider | Writable-root and guest-memory content, provider-managed lineage and content metadata |
| Shared contract | Provider instance UUID, public checkpoint UUID, opaque reference UUID, parent reference, source lease generation and provider-manifest digest |

The public checkpoint UUID and opaque reference UUID are deliberately distinct.
The first binds content to the catalog entry; the provider resolves the second
through its own identifier scheme. Blaze rejects a record when either identity
is nil, the public identity does not match its catalog location, or the digest
is not canonical SHA-256.

The reference has no per-resource ownership flags. Its presence means the
provider owns one complete, internally consistent writable-root and guest-memory
image. Blaze does not support or represent partial ownership because that would
require another atomic coordinator across independently committed payloads.

## Capture transaction

Blaze allocates the public checkpoint identity before any provider mutation and
uses it as the idempotency identity for capture. The order is:

1. verify the current checkpoint parent and allocate an unpublished catalog stage;
2. quiesce the backend and persist the paused operation boundary;
3. capture backend state while writes remain stopped;
4. ask the provider to freeze its data-plane content at the same boundary;
5. persist the advanced active lease and the provider reference as pending retirement;
6. atomically publish the public manifest and update the lifecycle journal;
7. remove the pending-retirement marker only after publication is durable; and
8. unquiesce the backend and complete the public operation.

An `OutcomeUnknown` capture is not repeated blindly. Blaze inspects the active
lease and accepts only the exact generation before the call or its exact
one-generation successor. It then asks the provider to retire content by the
preallocated public checkpoint identity, without inventing a provider object
ID. A known unpublished reference is retired by its complete public and
provider identity before the backend is resumed. If inspection or retirement
cannot be proven, the sandbox enters `RecoveryRequired` and the durable
write-ahead identity remains available for startup recovery.

## Restore transaction

Rollback never reuses the active lease or its devices. Blaze asks the provider
to prepare an independent replacement lease from the immutable reference while
the current backend is still running. The replacement is durably recorded
before the old backend is stopped.

After the old backend and old lease are stopped, Blaze starts the replacement
from the daemon-owned backend payload plus either a provider path-backed slot or
typed opened attachments. It validates backend identity and readiness before it
commits the replacement lease. Only then does it update checkpoint `HEAD`,
release the predecessor lease, atomically promote the replacement lease into
the public sandbox record, publish `Running`, and finalize provider ownership.

Any failure before the old backend is stopped aborts the replacement and leaves
the running sandbox unchanged. Any failure after that boundary cannot safely
reconstruct the former live instance; Blaze kills an untrusted replacement when
possible, retains both durable ownership records needed for cleanup, and marks
the sandbox `RecoveryRequired`. It never publishes a half-ready replacement as
running.

`RestoreCheckpointRequest` accepts a newly selected `RequestContext`; therefore
the provider contract can prepare independent leases for rollback or for a
future clone operation. The public HTTP API implements in-place rollback only.
A create-from-checkpoint endpoint and its policy contract are not implemented
and must not be advertised as supported.

## Retirement and deletion

Public reachability is authoritative. Pruning first computes and records all
provider references that may need retirement, then removes the unreachable
public catalog entries. Provider content is retired only for entries proven
removed. Each successful retirement clears its durable pending marker
individually, making retries idempotent after a crash.

Sandbox deletion follows the same ordering: inventory provider references,
persist the pending list, remove the public checkpoint namespace, and retire
provider content. A provider reference is never deleted while a public catalog
entry still owns it. Providers must additionally protect content referenced by
their own children or independently prepared leases.

## Provider requirements

An implementation of `DataPlaneCheckpoint` must:

- make capture idempotent by the preselected public checkpoint UUID;
- return an exact one-generation advance of the finalized active lease;
- freeze an immutable, internally consistent root-and-memory view;
- validate reference lineage and the canonical manifest digest on restore;
- return a new exclusive prepared lease rather than the source lease;
- make retirement idempotent by the complete provider instance, public
  checkpoint, and optional reference identity; accept the same deterministically
  derived operation identity after a daemon restart;
- preserve content until Blaze has removed the corresponding public owner.

The conformance crate validates provider-independent identities, transitions,
lineage, attachment shape, and retirement results. It cannot prove the contents
of a provider's content manifest or that a real backend observed one exact
write boundary.

## Verification boundary

Unit tests cover provider capture, rollback to an independent lease, pruning
after catalog removal, backend-only public payloads, identity validation, and
compensation when public publication fails. A production provider still needs
Linux integration and fault-injection evidence for concurrent captures,
multi-level lineage, daemon restarts at every durable boundary, multiple
restores from one reference, and deletion while descendants remain reachable.
