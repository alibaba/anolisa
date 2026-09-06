# Provider Data-plane Capacity

[中文版](provider-capacity_zh.md)

## Scope

The optional `DataPlaneCapacity` extension reports and drains reusable
data-plane resources owned by a build-time provider. It does not create or
manage reusable sandbox instances, backend processes, network namespaces,
processor allocations, or guest state. Each extension maps its resources into
provider-independent capacity classes exposed by the Blaze API.

Capacity is partitioned by `CapacityScope`, the exact pair of runtime backend
and provider-independent resource class that may consume the resources. A
`CapacityClass` declares the maximum root-filesystem and guest-memory extents
that every resource in that class can serve. These two portable requirements
fully determine the class across different provider implementations.

A zero capacity means that reusable resources in this class do not preallocate
that attachment role; both capacities cannot be zero. A later `prepare` may
still create a missing role through the same lease transaction, but it must not
count that cold resource as reusable capacity.

The class identifier is SHA-256 over the domain string
`blaze.data-plane-capacity.v1\0` followed by the two unsigned 64-bit capacities
in network byte order. Changing either requirement therefore produces a new
identity, while equivalent implementations produce the same identifier. A
provider must reject a scope it does not own instead of returning another
partition or silently falling back to file resources. A physical resource is
accounted in exactly one class even when it could satisfy a smaller class.

## Complete state partition

Every resource in one snapshot belongs to exactly one public state:

| State | Meaning |
|---|---|
| `ready` | Idle, fully verified, and safe for one new lease to claim |
| `building` | Still being created, cleaned, or verified; not claimable |
| `in_use` | Exclusively held by an active lease and not selected for drain |
| `draining` | No longer reusable and scheduled for removal as soon as its current work permits |
| `quarantined` | Retained outside allocation because ownership or cleanup is unresolved |

The states are mutually exclusive. An active resource selected by a drain
moves from `in_use` to `draining` even though it continues serving its existing
lease until release. A quarantined resource is not also counted as draining;
operator resolution must move it to a subsequent state.

`CapacitySnapshot::checked_total` is the checked sum of all five states. Blaze
rejects a snapshot with an incorrect provider or scope identity, both resource
capacities absent, a class digest that does not match the public requirements,
a zero revision, or count overflow. The provider owns monotonic revision allocation;
the public contract does not infer a revision from wall-clock time.

`accepting_allocations` states whether `prepare` may claim from the partition.
It is independent from the instantaneous ready count: a healthy empty class
may still accept cold preparation, while an accepted drain must set it to
false before returning.

## Claim and recycle boundary

The capacity extension has no public `claim` or `recycle` method. Those actions
remain part of the base provider lifecycle:

- `prepare` may atomically claim one `ready` resource and bind it to the new
  lease;
- `release` may return a fully cleaned resource to `ready`, remove it, or move
  it to `quarantined`; and
- a capacity shortage returns an explicit provider error and never selects a
  different data plane.

Keeping allocation in `prepare` preserves one lease transaction and prevents a
caller from claiming capacity that it cannot subsequently bind.

## Drain semantics

`drain` targets one exact scope and carries a stable `operation_id`. The
provider must make repeated calls with the same identity return the same
accepted result. After acceptance:

- idle `ready` resources are removed and reported as `removed_ready`;
- unfinished `building` resources are cancelled or marked `draining`;
- active `in_use` resources move to `draining`, continue serving their current
  lease, and are removed after release; and
- `quarantined` resources stay isolated for explicit operator handling.

The returned snapshot represents state after the drain was accepted.
`deferred_in_use` is the number of active resources whose removal must wait and
cannot exceed the returned `draining` count. Drain does not terminate a sandbox,
revoke a lease, or promise that every deferred resource has already
disappeared. The accepted snapshot has no `ready`, `building`, or `in_use`
resources and reports `accepting_allocations = false`; resources still serving
existing leases are counted only as `draining`.

An `OutcomeUnknown` result is retried with the same operation identity. A
provider must retain enough durable operation state to answer that retry after
its own restart. A second unknown result is returned to the caller rather than
being treated as success.

## HTTP surface

The daemon maps the optional extension to two scoped routes:

- `GET /v1/pools/{backend}/{class}` returns the public class requirements, five
  counts, their checked total, allocation state, and the provider revision;
- `POST /v1/pools/{backend}/{class}/drain` drains that partition. Its body may
  be empty or contain `{"operation_id":"<uuid>"}`. Supplying the identity is
  recommended when a caller needs to correlate retries.

`{class}` is the 64-character lowercase digest described above. Only the
daemon's active backend can be addressed. Malformed backends or class digests
return `400`; a valid but inactive backend or an unowned class returns `404`;
and a binary whose build-time provider does not implement `DataPlaneCapacity`
returns `501` without side effects. Responses use the provider-independent
class identity and aggregate resource states defined above.

`GET /v1/pools` and `PUT /v1/pools/{backend}/{class}/sizing` remain reserved
and return `501`. The current interface therefore neither invents
cross-partition inventory nor exposes a public resizing policy.

## Provider requirements

An implementation of `DataPlaneCapacity` must:

- maintain durable, mutually exclusive resource states for each scope and
  never count one physical resource in multiple classes;
- return one exact scope and provider identity with a monotonic nonzero
  revision;
- return the canonical public requirements whose digest selected the class;
- enforce exclusive claim as part of `prepare`;
- remove all old binding and writable data before a released resource becomes
  `ready` again;
- prevent `building`, `draining`, and `quarantined` resources from allocation;
- make drain idempotent by operation identity, stop new allocation before
  acknowledging it, and preserve active leases;
- complete deferred removal during later release or recovery; and
- report capacity exhaustion explicitly without creating an untracked
  resource or changing provider.

## Verification boundary

Public tests cover canonical class identity, snapshot accounting, exact scope
and provider identity, overflow rejection, unsupported standard-provider
behavior, the provider-independent HTTP response schema, inactive, unknown, and
malformed scope rejection, and an idempotent drain response.

The public tests cannot prove a concrete provider's resource cleanup, template
neutrality, concurrent claim exclusivity, restart recovery, or absence of data
from a previous lease. Those properties require provider integration tests on
the target operating system, including failure injection during build, claim,
bind, recycle, drain, and quarantine transitions.
