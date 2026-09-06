# Build-time Data-plane Providers

[中文版](build-time-data-plane-providers_zh.md)

## Purpose

Blaze exposes a source-level data-plane provider contract so downstream
developers can compose custom resource implementations with the daemon without
patching the Blaze source tree. The provider is selected when the final binary
is built. It is not discovered from a plugin directory, chosen by a tenant
request, or switched by the standard daemon configuration.

The standard `blazed` binary always uses the existing file implementation. A
downstream binary may depend on `blazed` as a library, implement
`DataPlaneProvider` in an extension crate, and pass that value to
`BlazeDaemonBuilder`.

## Compatibility boundary

This contract is a Rust source interface, not a stable dynamic-library ABI. A
provider binary must pin compatible revisions of:

- `blaze-provider-api`;
- `blaze-provider-conformance`;
- `blazed` and `blaze-core`;
- the Rust toolchain; and
- the dependency lock used for the final build.

`ProviderDescriptor.contract_version` protects the runtime boundary from an
obvious contract mismatch, but it does not replace source and dependency
pinning.

## Lifecycle-state namespace

The configured `daemon.state_dir` remains the process-coordination root. The
standard `blazed` binary stores lifecycle records directly below that root and
keeps its existing layout unchanged. A daemon composed with
`BlazeDaemonBuilder` instead uses this stable default state root:

```text
<daemon.state_dir>/.provider-state-v<contract_version>-<provider_instance_id>
```

The provider identity is written as a canonical, lowercase, hyphenated UUID.
The derived directory is a direct child whose complete name cannot itself be a
UUID. A standard daemon, including an older release that scans only immediate
UUID children of `daemon.state_dir`, therefore does not load, rewrite, or clean
provider-backed lifecycle records. Downgrade safety relies on this directory
boundary, not on how an older serializer handles fields it does not recognize.

Blaze validates the configured outer root first, validates the derived root
again before creating owned directories, and retains exclusive locks on both
the opened outer root and the selected namespace. The namespace is created and
opened relative to the retained outer-directory descriptor without following
a namespace symbolic link. The outer lock excludes two cooperating daemons
while the configured root's directory entry continues to name the retained
object.

The configured state root and its parent are therefore part of the daemon's
trusted host configuration. They must not be renamed or replaced while the
daemon is running, and an untrusted user must not be able to modify the parent
directory. Directory locks are attached to opened filesystem objects rather
than path strings: replacing the complete root can create a different object
with a different lock. The retained descriptors still prevent existing state
I/O from being redirected, but they cannot turn a replaceable path into a
host-wide singleton. Service supervision and directory permissions must enforce
this existing deployment requirement.

Before accepting a custom provider, startup inspects the standard root and
every other provider namespace without cleaning them. A non-terminal record,
an unfinished state publication, a malformed ownership record, or a concurrent
change stops startup. Another provider identity is accepted only when every
retained lifecycle record is a valid `Destroyed` record with no remaining
ownership. This check prevents changing `provider_instance_id` from silently
adopting or bypassing live state.

Composition authors must keep `provider_instance_id` stable for the lifetime
of the provider's ownership domain. Changing the identity or contract revision
selects a different namespace; Blaze neither migrates nor copies records
between namespaces. Complete cleanup with the original provider before an
intentional identity change. Likewise, running the standard binary after a
downgrade will not clean resources represented only in an extension namespace;
finish those lifecycles with a compatible extension binary first.

This namespace contains Blaze lifecycle records. It does not replace the
provider's own durable resource ledger, whose location and recovery rules
remain part of the extension's configuration and documentation.

The standard file provider keeps its ownership ledger in a protected sibling
directory below the configured instances root, outside every removable sandbox
directory. It writes a complete prepare identity before creating a slot, marks
the record ready only after the artifacts are durable, and marks it deleting
before recursive removal. The record is removed only after the empty slot and
its parent have been synchronized. Startup therefore can resume every
interrupted allocation or removal without treating a same-named, unrecorded
directory as owned. It also removes only strictly named, unpublished temporary
records left by an interrupted atomic ledger update; other ledger entries are
not treated as disposable.

The file provider's `provider_instance_id` is a deterministic UUID derived from
the canonical instances-root object, including its filesystem device and inode.
Reconstructing the provider over the same root preserves its identity, while
selecting a different or replaced root produces a different identity. An old
binding therefore conflicts before lookup or deletion instead of being reported
as released merely because the newly configured root is empty. This
root-derived descriptor follows the same stability rule that the build-time
lifecycle-state namespace mechanism requires from composition providers.

Each ready ledger claim records the device and inode read from the opened slot
directory. Allocation never treats a random token or the slot pathname as
ownership evidence. If another directory wins the final name before `mkdir`,
the preparing claim remains unidentified and cleanup refuses to touch that
directory. A crash between `mkdir` and identity publication similarly retains
the ambiguous directory for operator investigation. Inspection and cleanup
compare the linked directory with the recorded object before reading or
removing it.

Restart lookup scans every canonical ownership manifest so reuse of either an
instance identifier or lease identifier with a different complete context is a
typed provider conflict. Exact idempotent preparation also compares source
fingerprint, logical root-filesystem and memory extents, and template VM-state
length. Before a ready claim is accepted, the provider verifies rootfs and
memory logical lengths; template claims additionally require a plain `backend`
directory with `vmstate.snap` and `memory.snap` of the recorded lengths.
Ordinary base-image copies must already match the requested logical extent.

Initial generations must leave room for the complete create-to-release
sequence. Subsequent transitions compute their next generation before any
cleanup begins, so overflow is a conflict with retained resources rather than
an unrepeatable deletion outcome.

The file provider retains a non-blocking exclusive lock on the opened instances
root. This is a cooperative single-writer contract, not protection from a
privileged host administrator: every writer to that root must use the same
lock, and deployment permissions must prevent unrelated processes from
changing the root, ledger, or slot trees while Blaze is running.
The instances root itself is rejected when group or other users can write it;
the ownership-ledger directory permits only owner access.

## Reproducible and privacy-preserving builds

Rust toolchains may retain absolute source paths in diagnostics and metadata.
For reproducible distribution builds, use a stable source root or remap build
paths with rustc's `--remap-path-prefix`. Apply the same rule to Cargo dependency
paths. Package only tracked source and declared release artifacts; do not copy
`.git`, `target`, local configuration, or generated test output.

These checks protect reproducibility, developer privacy, and deployment
secrets; they are not a substitute for documenting shipped behavior. Before
publication, compare the archive manifest with the declared release inputs and
inspect printable binary strings for unexpected local paths, host identifiers,
credentials, and undeclared configuration. Document intentionally shipped
product identifiers and settings as part of the extension release.

## Lifecycle contract

The base contract revision covers sandbox creation and deletion. Every
mutation is bound to a preselected sandbox, request, operation, lease, and
generation. A successful lifecycle follows this sequence:

| Operation | Required meaning |
|---|---|
| `probe` | Check prerequisites without allocating sandbox resources. |
| `prepare` | Create one provider-owned lease and return path-backed or opened restore resources. |
| `inspect` | Observe the exact state of a known lease without mutating it. |
| `commit` | Accept that the backend reached readiness before public state is published. |
| `finalize` | Close the handoff after the matching public state transition is durable. |
| `stop` | Record that backend use ended while retaining cleanup ownership. |
| `release` | Prove that all resources owned by the stopped lease are absent. |
| `abort` | Compensate a prepared or committed lease that has no durable public owner. |

Each successful transition keeps the same provider, request, operation, and
lease identities and increments the generation exactly once. An operation with
an uncertain result returns `OutcomeUnknown`; Blaze then uses `inspect` before
deciding whether compensation is safe.

Preparation may return either:

- `PreparedResources::PathBacked`, which preserves the existing file storage
  layout; or
- `PreparedResources::OpenedRestore`, which transfers typed root-drive and
  guest-memory descriptors for a validated template restore.

Opened resources are accepted only when the provider declares the corresponding
capability. Blaze validates the returned lease and resource shape before a
backend is started. If the resource shape is invalid but the binding is safe to
identify, Blaze calls `abort`; it does not compensate through an untrusted
binding.

## Composition

An extension crate implements `DataPlaneProvider`, defines its resource
configuration and durable state, and maps implementation errors to the
provider-independent `ProviderError` categories. A purpose-built command
binary is the composition root. The repository contains two executable
examples:

- [`minimal_provider.rs`](../../crates/blaze-provider-conformance/examples/minimal_provider.rs)
  runs the reusable create-and-delete exercise against a complete base-trait
  implementation;
- [`custom_provider_daemon.rs`](../../crates/blazed/examples/custom_provider_daemon.rs)
  passes that provider to `BlazeDaemonBuilder` and starts the daemon.

Build and inspect both entry points from `src/blaze`:

```bash
cargo run -p blaze-provider-conformance --example minimal_provider --locked
cargo run -p blazed --example custom_provider_daemon --locked -- --help
```

The daemon example accepts `--daemon-config <path>` and
`--resource-root <absolute-directory>` directly; it does not use the standard
binary's `daemon start` subcommand. The shared `ExampleFileProvider` creates
real sparse file resources, but deliberately omits persistence and all optional
extensions. Production providers must additionally test their real backend,
compensation, concurrency, and failure recovery.

The builder validates the provider descriptor and runs `probe` before Blaze
creates daemon-owned directories. A probe failure stops startup. It never
constructs the standard file provider as a replacement for a failed build-time
provider.

## Extension configuration

`DaemonConfig` remains provider-independent and does not select a build-time
extension at runtime. Each extension defines the configuration and resource
mapping it needs in its crate and composition binary. Values returned across
the Blaze contract are limited to public, implementation-neutral types and the
stable `ProviderError` categories; Blaze does not ingest arbitrary provider
error strings. Extension code may emit additional diagnostics according to the
deployer's logging policy.

Names returned through the management interface are part of the extension's
versioned product contract and should be documented by its maintainer.
Transport endpoints, resource mappings, and provider-specific settings stay in
the provider package and its own operator documentation so the Blaze
configuration remains portable across independent implementations.

Blaze uses explicit management representations for sandboxes and checkpoint
manifests. They contain the documented lifecycle, policy, backend, and artifact
fields, but not data-plane leases, recovery records, or provider checkpoint
references. Selecting a provider at build time therefore does not silently
extend the HTTP response schema.

Contributions to Blaze itself follow the same reusable-contract rule. Public
types, comments, examples, fixtures, and diagnostics describe observable
roles, capabilities, lifecycle results, and stable error categories. Examples
must be understandable and executable from the tracked Blaze sources alone.
Provider-specific resource topology and configuration are defined and
documented by the provider that owns them.

The existing `[storage]` file directories remain required for file-backed
sandboxes and daemon-owned catalogs. A primary provider changes creation and
deletion through the base contract. It may declare `daemon_managed_storage` to
use the configured storage provider for checkpoint, hibernation, and restore,
or separately opt into provider-owned lifecycle extensions. A provider lease
alone does not require or silently acquire either optional extension.

## Supported and deferred behavior

| Scenario | Current status and prerequisites |
|---|---|
| Ordinary image creation with path-backed resources | Supported |
| Template creation with path-backed resources | Supported |
| Template creation with opened root-drive and guest-memory resources | Requires both a provider declaration and the Firecracker restore adapter, which is currently the only consumer |
| Ordinary image creation with opened restore resources | Rejected before backend start |
| Failed compiled-provider probe | Startup fails; no file fallback |
| Provider lease adoption after daemon restart | Requires the optional inventory contract and a backend that supports identity-based adoption |
| Provider-owned checkpoint and rollback | Requires the optional checkpoint contract and full backend snapshot and restore support |
| Create a new sandbox from a provider checkpoint | Deferred; no public endpoint or policy contract |
| Provider-owned hibernation and resume | Requires the optional suspension contract, full backend snapshot and restore support, and the documented guest protocol operations |
| Provider-owned reusable data-plane capacity and drain | Supported through the optional capacity contract |
| Reusable backend, network, and complete sandbox pools | Not supported |
| Runtime dynamic-library or process plugin discovery | Not supported |

A provider that does not support restart adoption must not be presented as
production-ready for persistent workloads. The inventory contract and its
fail-closed recovery rules are specified in
[Provider Reconciliation](provider-reconciliation.md). Provider checkpoint
ownership and compensation are specified in
[Provider-owned Checkpoints](provider-checkpoints.md). Suspension ownership,
guest recovery hooks, and failure handling are specified in
[Provider-owned Suspension](provider-suspension.md). Reusable data-plane
capacity reporting and drain behavior are specified in
[Provider Data-plane Capacity](provider-capacity.md); this does not provide a
complete reusable-sandbox pool.

## Verification

At minimum, an extension should:

1. run `blaze-provider-conformance::exercise_create_delete` against isolated
   resources;
2. test every uncertain-result and compensation branch;
3. prove that unsupported sources fail before side effects and do not select a
   different provider;
4. verify that a real backend consumes the exact resources returned by the
   lease;
5. verify that deletion leaves no active backend, attachment, provider resource,
   or claimable lease; durable idempotency and tombstone records may remain only
   when they no longer own resources; and
6. verify that Blaze-facing APIs, logs, metrics, and conformance evidence follow
   the public contract, while documenting the extension's own resource model in
   its developer and operator materials.

The conformance crate validates the provider-independent state and resource
shape. A concrete extension still has to prove its own correctness and
production readiness.
