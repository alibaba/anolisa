# Provider Reconciliation

[中文版](provider-reconciliation_zh.md)

## Scope

Blaze can adopt a provider-owned sandbox after daemon restart only when the
build-time provider exposes `DataPlaneInventory` and all three ownership ledgers
agree:

1. the durable public sandbox record;
2. one immutable provider inventory snapshot; and
3. the live backend identity.

This is an optional extension of `DataPlaneProvider`. A provider without it
uses the standard fail-closed cleanup path and is never silently treated as
adoptable.

## Durable ownership

Every provider-backed sandbox record stores the provider instance, request,
operation, lease, initial generation, current generation, lease state, and the
root-filesystem and guest-memory extents. Blaze persists `Prepared`,
`Committed`, `Finalized`, `Stopped`, and `Released` transitions as they occur.

Before public `Running` is published, Blaze also persists the backend runtime
evidence needed for adoption. For Firecracker this includes the PID and Linux
process start-time tick, the frozen backend version, and whether the guest
transport, network slot, and console log are owned. A PID alone is not an
identity because the kernel may reuse it.

## Inventory contract

`begin_inventory` freezes one provider view. Blaze then reads bounded pages
from that snapshot and rejects:

- a snapshot from another provider instance;
- nil or changed identities;
- a duplicate lease identity;
- an oversized page, repeated cursor, or more than the global safety bound;
- a generation older than the initial generation.

Invalid inventory is a startup error. Blaze does not open its API socket with
an incomplete or self-contradictory provider view.

More than one lease may temporarily name the same sandbox during a durably
recorded replacement transaction. Such leases are not interchangeable: only
the exact active or replacement lease in the public ledger can be matched;
every unexplained extra lease is quarantined.

## Adoption and quarantine

Blaze adopts only a public `Running` sandbox with no unfinished operation, a
durable running backend, an exact matching `Committed` or `Finalized` lease,
and complete backend runtime evidence. Firecracker adoption additionally
proves the process start time, sandbox environment identity, PID handoff file,
API responsiveness, frozen version, and network namespace ownership before
the process is retained.

After backend and guest readiness are proved, Blaze asks the provider to adopt
the exact lease. The provider must preserve every identity, advance exactly one
generation, and return `Finalized`. Blaze persists that result before serving
the sandbox.

Startup first settles durable transition journals, retries obsolete content
retirement, and cleans known interrupted lifecycle transactions. Only a real
mismatch that remains after those steps is fail-closed: Blaze asks the provider
to quarantine retained resources, stops a proven backend owner when safe, and
marks the public sandbox `RecoveryRequired`. A provider-only lease is also
quarantined. Blaze never guesses ownership, adopts by path, or releases a
resource through an untrusted identity.

## Crash boundaries

The ordering is deliberately asymmetric:

| Boundary | Durable evidence after a crash | Restart action |
|---|---|---|
| Provider prepared, backend not started | Prepared lease and operation journal | Settle the journal and clean up; never adopt |
| Backend started, public state not running | Lease plus backend identity | Settle the known transaction and clean up; never publish running |
| Public running, provider committed | Exact committed lease and backend identity | Complete adoption to finalized |
| Public running, provider finalized | Exact finalized lease and backend identity | Re-adopt with a new generation |
| Provider adoption succeeded but public persistence failed | Exact adoption before-image and target are in the transition WAL | Inspect, persist the exact successor, and resume adoption |

`OutcomeUnknown` is not success. Implementations must retain enough evidence
for a later `inspect` or inventory traversal to resolve it.

## Verification boundary

The conformance crate validates inventory identities and exact reconciliation
transitions. Unit tests cover finalized adoption, adoption from the committed
crash boundary, and identity-drift quarantine. A production provider must also
prove restart behavior with its real resources and a real Linux backend,
including process reuse, stale network state, lost responses, and repeated
reconciliation.
