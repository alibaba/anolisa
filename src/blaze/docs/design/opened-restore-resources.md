# Opened Restore Resources

[中文版](opened-restore-resources_zh.md)

## Context

Firecracker normally restores a captured virtual machine by reopening root-drive
and guest-memory paths. Reopening a path can select a different object after a
rename or replacement, and it prevents a storage implementation from handing
Blaze an object that has already been opened and verified.

Blaze therefore accepts an optional collection of typed, opened attachments on
the restore request. The ordinary file-backed path remains the default and is
unchanged when no attachment collection is supplied.

`ProviderCapabilities` declares this permission independently through
`opened_template_restore_resources`,
`opened_checkpoint_restore_resources`, and
`opened_suspension_restore_resources`. Each flag means the corresponding
template preparation, checkpoint restore, or suspension resume may return
opened attachments; it does not require every successful call to do so. A
declaration for one operation never authorizes opened attachments from either
of the other two operations.

## Contract

Each collection is bound to one sandbox, one nonzero lease identifier, and one
positive lease generation. Each attachment declares:

- a unique backend role: root drive or guest memory;
- one read-write descriptor transferred exclusively to a single backend owner;
- regular-file, character-device, or block-device object kind;
- a nonzero, page-aligned logical extent; and
- a pre-provisioned consumer path when the captured root drive requires one.

Before starting Firecracker, Blaze compares every declaration with facts read
from the opened descriptor. It rejects duplicate roles, stale sandbox bindings,
descriptor aliasing between roles, access or object-kind mismatches, invalid
logical extents, and descriptors that are not open for both reading and writing.

On Linux, Blaze preserves only the approved descriptors across `exec`. The root
drive is bound to its captured path inside the child process's isolated mount
namespace, while the guest-memory descriptor is passed to Firecracker as
`/proc/self/fd/<number>`.
Blaze retains the attachment collection for the lifetime of the backend owner,
so cleanup cannot close a descriptor while Firecracker still depends on it.

## Scope

This contract changes only Firecracker restore input and ownership. Provider
selection and resource leases are defined by
[Build-time Data-plane Providers](build-time-data-plane-providers.md). Restart
adoption, checkpoint ownership, suspension, and reusable capacity are separate
optional contracts described in [Provider Reconciliation](provider-reconciliation.md),
[Provider-owned Checkpoints](provider-checkpoints.md),
[Provider-owned Suspension](provider-suspension.md), and
[Provider Data-plane Capacity](provider-capacity.md).

The Firecracker restore adapter is currently the only backend adapter that
declares support for these attachments. For each selected operation, Blaze
checks the matching capability before provider mutation and rejects the request
when the backend cannot consume attachments that the provider may return. After
the provider call, the daemon and conformance library both reject opened
attachments unless that same operation-specific capability was declared.
Path-backed results remain valid when the capability is declared.
