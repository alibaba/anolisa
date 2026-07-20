# Install Lifecycle Design

[中文版](install-lifecycle_zh.md)

This document defines the authority, scope, planning, transaction, and recovery
model shared by the `anolisa` component lifecycle commands. The model exists to
keep ANOLISA state honest when raw files, native packages, concurrent commands,
or process interruption change the machine independently.

## Design Goals

- Separate ownership from distribution format. Raw artifacts are normally
  ANOLISA-owned; RPM installations remain authoritative in rpmdb.
- Make every lifecycle decision from facts observed in one scope and one lock
  epoch.
- Persist enough intent before side effects for deterministic crash recovery.
- Preserve system-install visibility for regular users without granting a
  user-scoped command permission to mutate system state.
- Refuse ambiguous recovery instead of guessing a package, subject, or owner.

## Authority Model

Each active installation has exactly one `ProviderBinding`:

| Binding | Source of truth | Mutation authority |
|---------|-----------------|--------------------|
| `Owned` | ANOLISA record and recorded file hashes | ANOLISA may verify, replace, or remove its files |
| `Delegated` | Native package database | ANOLISA may observe; native transactions depend on the management relation |

A delegated binding also records one relation:

| Relation | Meaning | Default uninstall |
|----------|---------|-------------------|
| `Managed` | ANOLISA installed the package | Delegate package removal to the native manager |
| `Adopted` | The user explicitly adopted a pre-existing package | Drop the ANOLISA record only |
| `Observed` | The package is visible without management consent | Drop the ANOLISA record only |

`--remove-system-package` is explicit, per-invocation authority to remove an
adopted or observed package. It is not persisted as a new ownership relation.

## Scope And Visibility

User and system installations have separate layouts, state files, locks, and
journal directories. A mutating command writes only the layout selected by
`--install-mode` (or the UID-derived default).

Read-only state projection is deliberately wider: `list`, `status`, `doctor`,
and adapter discovery can combine readable user and system roots. This lets a
regular user use and inspect a system installation. It does not merge the two
records or transfer authority between them.

The resulting invariants are:

1. A system installation does not block
   `anolisa --install-mode user install <component>`.
2. A user-scoped `forget`, `uninstall`, `update`, or `repair` never mutates the
   visible system record.
3. In a user view with both records, the user record is active and the system
   record is reported as shadowed. System mode reads only the system root.
4. System mutations still require the privileges of the underlying operation.

## Planning Pipeline

Every lifecycle handler follows the same boundary:

```text
request -> scoped facts -> pure planner -> typed steps -> executor -> record
```

Facts contain the selected-scope record, native observation when relevant,
owned-file integrity, adapter claims, quarantine state, and effective pending
journal status. The planner is a pure decision table; it cannot write files,
run dnf, or update state. Executors interpret only their own step family.

| Intent | Important decision |
|--------|--------------------|
| `install` | Install into the selected scope; never treat another scope as the same record |
| `update` | Refresh owned artifacts or delegate a managed package update |
| `uninstall` | Remove only what the binding and relation authorize |
| `adopt` | Convert a present system package into a delegated-adopted record |
| `repair` | Re-observe authority, recover a journal, or replay verified owned state |
| `forget` | Drop only the selected-scope record, with no package or file side effect |

Planning runs again after acquiring the scope's install lock. This closes the
gap in which another lifecycle command could change the record, package
identity, adapter claims, or pending-journal gate after the first read.

## Transaction Protocol

A journal is created in the same state root as the record it protects. Each
step is recorded as `Planned` before its side effect and transitions to a
terminal step status after execution.

Delegated operations persist the per-subject recovery context and their first
step batch in one atomic journal revision. The context binds:

- the exact component subject;
- native package manager and resolved package, when one exists;
- the intended record transition.

This prevents a crash from exposing a recovery identity with no evidence of
which side effects were planned. Batch install and update use one shared native
transaction but retain one journal and recovery identity per component.

Native package transactions are forward-only. Once dnf may have committed,
ANOLISA does not guess at an inverse operation; failures remain `Partial` and
repair re-observes rpmdb. Owned operations use reverse-order compensation.
Backup content is hash-checked and restored through an atomic replacement that
does not follow a destination leaf symlink.

## Recovery Classification

Recovery first loads a `JournalInventory` for the entire selected state root.
Every journal path, schema, state binding, and same-root operation record must
validate before any entry is consumed. Recovery then uses the already-validated
in-memory transaction, avoiding a second path load after validation.

New delegated journals require an exact subject and a non-empty atomic intent.
Before a recovered record write or drop, repair revalidates the current record,
package identity, package manager, and management relation. In particular, a
managed record cannot be dropped while its package is still present unless the
journal records the corresponding native removal.

Pre-refactor RPM install journals remain supported through a narrow legacy
classifier. A live legacy claim must have the exact known install/state marker
shape and unambiguous package/component identities. A journal carrying a new
subject without the new recovery context is neither accepted as legacy nor
silently replanned; it remains pending with an actionable safety error.

Settled legacy journals are ignored only when compatible operation history in
the same state root proves that their effect was committed. Evidence never
crosses user/system roots.

## Failure Policy

- Invalid or ambiguous facts fail before side effects.
- A malformed pending journal remains pending for inspection.
- Native side effects without a committed record are `Partial`, not clean
  failures.
- Record-only adopted/observed uninstall remains valid while the package is
  present.
- Managed record-only uninstall is valid only after the package is observed
  absent.
- Recovery never overwrites a record whose authority or package binding has
  changed since the journal was written.

## Verification

Unit tests cover every planner decision row, executor step ordering, scoped
visibility and mutation, legacy journal classification, batch recovery, record
authorization, and owned rollback. The smoke suite exercises the compiled CLI
against isolated user/system roots and a real RPM database fixture.
