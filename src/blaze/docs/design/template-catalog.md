# Template Catalog

[中文版](template-catalog_zh.md)

The daemon can publish a reusable runtime artifact set into the directory
configured by `template.dir`. Imports are disabled unless an operator
also configures `template.import_root`.

`/v1/templates` is the daemon's single public template resource. The catalog
provides durable publication and lookup, but sandbox creation does not yet
select catalog entries.

A later sandbox-create change will accept an optional template name and resolve
that name against this catalog. It will not add a second template registry or
another template API namespace.

## Import request

```http
POST /v1/templates/import
Content-Type: application/json

{
  "name": "runtime-base",
  "source": "runtime-base",
  "description": "base runtime"
}
```

`source` is a relative path below the configured import root. Absolute paths,
parent traversal, and symbolic links in the path are rejected. Every source
directory and file must be owned by the daemon user and must not be writable
by group or other users.

The source must contain top-level regular files named `vmstate.snap`,
`mem.bin`, and `rootfs.ext4`. An optional `template.json` must contain a JSON
object. Nested directories, links, and special files are rejected. The daemon
sets `name` from the request, applies a non-empty request description, and
fills `rootfs_size` and `memory_size` defaults when either field is absent or
is not an unsigned integer.
It returns `409 Conflict` when the destination exists or another import of the
same name is active.

## Limits and owned paths

The following settings bound work before data is published:

| Setting | Meaning |
|---------|---------|
| `max_files` | Maximum files in one published entry, including `template.json` |
| `max_bytes` | Maximum artifact and generated metadata bytes in one entry |
| `max_metadata_bytes` | Maximum input and generated metadata size |
| `max_total_bytes` | Maximum committed bytes plus concurrent reservations |
| `max_entries` | Maximum committed entries plus concurrent import reservations |

`template.dir` and `template.import_root` must be absolute,
must not contain parent components, and must not overlap each other. They also
must not overlap the storage image, storage instance, or configured policy
directories, any executable path configured in `[backends]`, the
resolved location captured when the daemon configuration file is opened for
this startup, that file's configured pathname, the `daemon.socket` path, or the
host network coordination path `/run/lock/blaze-network.lock`. The conventional
named network namespace trees `/var/run/netns` and `/run/netns` are protected
as well.
Relative `[backends]` paths are
resolved once against the daemon's startup working directory, and that absolute
path is reused for boundary validation, probing, and launch. For a configured
backend symbolic link, both the link location and its resolved target are
protected from template catalog ownership. The configured and resolved
locations of a symbolic-link daemon configuration path receive the same
protection. Startup resolves
existing path prefixes, compares their underlying Linux filesystem locations
using the mount table, and rejects symbolic-link components in either
template catalog root. It retains the opened configuration file and repeats its
identity check at the captured location while validating boundaries.
Symbolic-link and bind-mounted aliases therefore cannot bypass the ownership
boundaries or redirect catalog setup. An overlap is rejected before catalog
permissions are changed or published entries are scanned.
For a catalog root that does not exist yet, startup retains the deepest
existing parent directory and creates the planned missing components relative
to that descriptor. If a planned component appears during boundary validation,
startup rejects it instead of adopting or changing the new object. Policy-entry
boundary discovery follows `policy.on_load_error`: a discovery failure in
`warn` mode contributes no entry targets because policy loading will use an
empty engine, while every successfully discovered target remains protected.
Startup also resolves every executable candidate for Blaze's host helper
commands from `PATH` and protects each configured location and resolved target
from catalog ownership.
When imports are enabled, startup opens and retains the validated import-root
directory. Each source lookup begins from that retained directory object, so
replacing the configured pathname after startup does not redirect imports.
The roots may use non-UUID children of `daemon.state_dir`, including the
default catalog directory, but cannot own the state root or enter a sandbox
UUID subtree.

The catalog, staging directories, and published directories use mode `0700`.
Published files use mode `0600` and must have exactly one hard link. Catalog
entries and staging directories must stay on the catalog root's mount; startup
and API reads stop if an entry crosses a nested mount boundary.
Startup scans and API reads classify each artifact before obtaining a
read-capable handle and recheck the opened object's identity. On Linux, a
metadata-only handle pins the classified object and supplies its readable
handle, so a pathname replacement cannot redirect inspection.

## Publication and recovery

The importer opens source entries without following links, reserves catalog
capacity, and copies them into a private, uniquely named staging directory.
It checks the source identity and size again after copying. The complete
directory is synchronized and renamed into place without replacing an
existing entry, so readers see either no entry or the complete entry.

A failed import removes its staging directory, including failures after the
directory is created but before it can be opened and validated. If cleanup
cannot be completed, or publication has occurred but catalog durability cannot
be confirmed, the daemon rejects later imports until the catalog is repaired
and the daemon restarts. Startup acquires and retains an exclusive lock on the
opened catalog root before scanning entries or removing owned staging
directories left by an interrupted run. A second daemon using the same catalog
therefore fails before it can interfere with an active import. The lock owner
validates the type, ownership, permissions, contents, and capacity of published
entries.

During graceful shutdown, the daemon rejects new imports, requests
cancellation of active imports, waits for their file handles and staging data
to be released, and then returns from the service loop. Draining already
accepted connections and releasing daemon-wide runtime resources are separate
shutdown responsibilities.

## Lookup and current limits

Published entries are available through:

- `GET /v1/templates`
- `GET /v1/templates/{name}`

The collection route returns sorted, name-only summaries. It validates one
entry at a time and discards that entry's full metadata before reading the
next; the item route returns the complete metadata. The collection response is
bounded by `max_entries` and the 128-byte template-name limit. Only one list
response may remain in flight, and a concurrent list request receives `503
Service Unavailable` until that response body is released. A separate
single-flight permit covers complete item parsing and the returned item body;
another item request receives `503 Service Unavailable` until the retained body
is released. Corrupt published metadata is reported instead of silently
hidden. These routes manage stored artifacts only. Validation is structural;
it does not prove that a snapshot is bootable or compatible with a particular
backend. Sandbox creation does not yet accept a template name, and the catalog
does not yet expose deletion or reference tracking.

The catalog limits above apply to imported artifacts and metadata. This change
does not add a daemon-wide HTTP request-body limit; that input boundary must be
provided separately before a production release.
