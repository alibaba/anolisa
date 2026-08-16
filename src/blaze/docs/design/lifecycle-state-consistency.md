# Lifecycle State Consistency and Compatibility

[中文版](lifecycle-state-consistency_zh.md)

Blaze has two related lifecycle boundaries. Before serving requests, it must
reconstruct a complete persisted sandbox inventory without exposing a partial
result. While serving requests, it exposes lifecycle and guest operations only
through the sandbox namespace and rejects reserved reusable-capacity operations
before they can change ownership. Retired `Reset`, `Warm`, and
`start_path = "warm"` values remain decodable so startup can clean non-terminal
records that contain them.

This document defines both boundaries. The inventory-publication protocol does
not change the HTTP API, configuration keys, or persisted JSON format. The
management API section defines the sandbox namespace and the reserved
reusable-capacity boundary.

## Terms and owned objects

The **state root** is the directory configured by `daemon.state_dir`. Each
persisted sandbox record is stored in one canonically named UUID directory
below that root. Its `state.json` file contains the lifecycle record used
during restart.

`StateStore` is the supported entry point for lifecycle-record persistence. It
keeps the opened state-root directory object for its lifetime instead of
reopening the configured pathname. For each active sandbox, it also retains an
opened UUID directory object. Later record and runtime-directory operations are
derived from these opened objects.

The **startup inventory** contains the validated lifecycle record for every
UUID-owned directory. A separate retained-owner map keeps the opened UUID
directories that the daemon must continue to use for later lifecycle and
backend operations.

## Writer coordination

A production daemon takes a non-blocking exclusive advisory lock on the opened
state root before it scans lifecycle records. Another Blaze daemon following
the same protocol cannot start with that state root until the first daemon
releases the lock.

Inside one daemon, the startup scan holds the `StateStore` run-directory map
lock for the complete scan and publication sequence. Lifecycle persistence
also enters this map through `StateStore`, so a supported in-process writer
cannot publish or release an owner while startup is constructing the
inventory. Per-sandbox record writes have an additional writer lock.

These two locks serve different purposes: the state-root lock coordinates
cooperating daemon processes, while the run-directory map lock coordinates
writers inside one daemon.

## Startup publication protocol

Startup follows this order:

1. Open the configured state root, take its advisory lock, and retain that
   opened directory object.
2. Enumerate UUID-owned entries and build private instance and retained-owner
   maps. For every UUID entry, require:
   - a canonical lowercase, hyphenated UUID directory name;
   - a directory rather than a link or another filesystem object, and the same
     directory object observed during enumeration;
   - a regular `state.json` with exactly one hard link, opened relative to that
     directory instead of through a replacement path;
   - a record whose sandbox ID matches the directory name; and
   - for `Destroyed`, no active operation and backend ownership of
     `NotStarted` or `Stopped`.
3. Complete a second enumeration of canonical UUID names and compare its full
   set with the first scan.
4. After the second enumeration has finished, revalidate every retained UUID
   directory and `state.json` against the objects accepted by the first scan.
5. Only after every check succeeds, publish the retained-owner map and return
   the instance map to `ServerState`.
6. Reconcile the accepted sandbox records, then bind the configured Unix and
   TCP API listeners.

The name-set comparison must finish before object revalidation begins. This
ordering prevents an early owner from being accepted while the final directory
enumeration is still processing later UUID entries.

## Failure behavior

Any missing, malformed, unexpectedly typed, aliased, or internally
inconsistent UUID record stops daemon startup. If the final name-set comparison
or object revalidation detects an added, removed, or replaced owner or record,
startup also stops. The scan does not publish a partial retained-owner map, and
the daemon does not open its API listeners.

Blaze leaves a rejected UUID directory and its `state.json` unchanged for
operator inspection and repair. Existing cleanup of state-publication staging
entries remains separate from rejected-record handling.

After a complete inventory has been accepted, startup reconciliation processes
each non-terminal sandbox independently. A cleanup failure for one sandbox can
retain that sandbox in memory as `RecoveryRequired` without turning the already
validated inventory into a partial one. Blaze attempts to persist the recovery
state; if that write also fails, reconciliation reports the additional error
and the durable record may still contain its previous state.

## Management API and reusable-state boundary

Lifecycle and guest operations are registered under `/v1/sandboxes`.
Action-style reset, checkpoint, and destroy paths are unregistered and return
`404 Not Found`. Canonical destruction remains
`DELETE /v1/sandboxes/{id}`. Checkpoint capture is defined separately.

The following reserved management routes also return `501 Not Implemented` and
do not manage reusable capacity:

- `GET /v1/pools`;
- `GET /v1/pools/{backend}/{class}`;
- `POST /v1/pools/{backend}/{class}/drain`; and
- `PUT /v1/pools/{backend}/{class}/sizing`.

`GET /v1/health` retains its `storage_pool` object for response compatibility;
the file provider reports zero ready, capacity, pending, and quarantined slots.
The metrics endpoint does not publish a reset counter because no reset route is
registered. Pool-hit and pool-miss counters also remain absent because reusable
capacity has no supported success path.

New sandbox creation always records `start_path = "cold"`. Lifecycle
transitions cannot enter `Reset` or `Warm`, so no supported path can produce or
reactivate a reusable sandbox. Blaze retains decoding of legacy `Reset`, `Warm`, and
`start_path = "warm"` values only so startup can release resources owned by
records written by earlier releases. After the complete inventory passes
validation, reconciliation destroys each such non-terminal record. Successful
cleanup reaches `Destroyed`. Failed cleanup retains the in-memory record as
`RecoveryRequired` and attempts to persist that state; a persistence failure is
reported and may leave the prior durable state intact. Reconciliation continues
with other accepted records. Create requests never select or reactivate a
legacy record.

## Consistency boundary

This protocol covers lifecycle-state writers that use `StateStore` and daemon
processes that participate in the state-root advisory lock. The advisory lock
does not prevent an unrelated process from modifying the directory directly.
A finite sequence of directory scans cannot provide an atomic snapshot against
such a writer.

Direct modification that bypasses the state-root lock is unsupported. Stronger
isolation for that path is tracked in
[#2459](https://github.com/alibaba/anolisa/issues/2459).

## Maintainer invariants

Future lifecycle-state changes must preserve these rules:

- production lifecycle writes go through `StateStore`;
- the state-root owner is acquired before inventory scanning and retained
  while request handlers can write lifecycle state;
- startup holds the run-directory map lock until the complete inventory is
  accepted or rejected;
- the final UUID enumeration completes before retained objects are
  revalidated;
- no request handler can observe either startup map before all inventory
  checks have passed;
- unregistered sandbox action routes return `404` before reading or changing
  sandbox state;
- pool-management rejections occur before lifecycle, runtime, or storage
  ownership changes; and
- lifecycle operations cannot enter or reactivate `Reset` or `Warm`; legacy
  values are cleanup inputs only.
