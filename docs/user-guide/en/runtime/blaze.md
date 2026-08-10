# Blaze Firecracker Networking

[中文版](../../zh/runtime/blaze.md)

Blaze can give each Firecracker sandbox a dedicated network namespace, tap
device, veth pair, and address slot. This capability is opt-in and is disabled
by default.

## Prerequisites

The Blaze daemon must run on Linux with permission to manage host networking.
The `ip`, `sysctl`, and `iptables` commands must be installed and executable.
Firecracker and its kernel and root filesystem images must also be available.

Blaze checks these prerequisites when a loaded policy both enables networking
and selects Firecracker as an eligible backend. Policies that leave networking
disabled do not require these host capabilities.

## Configuration

Set `enable_network` in the Firecracker section of a workload policy:

```toml
[select]
backend_priority = ["firecracker"]

[backend.firecracker]
enable_network = true
```

The option applies only to Firecracker. Its default is `false`, so existing
policies retain their previous behavior until they opt in.

## Runtime Behavior

When a request selects a network-enabled Firecracker policy, sandbox creation:

1. allocates a host-wide network slot;
2. creates an owner-qualified network namespace;
3. creates the tap and veth devices and configures addresses, forwarding, and
   namespace-local NAT; and
4. starts Firecracker with the tap device attached.

Allocation and deletion use `/run/lock/blaze-network.lock`, which prevents two
Blaze daemon processes on the same host from choosing the same slot at the same
time. Blaze records the namespace owner before creating dependent devices so a
partially completed setup remains attributable to the sandbox.

Explicit sandbox destruction removes the owned namespace and devices after the
backend process has stopped. A compensated startup failure performs the same
cleanup. If cleanup cannot be confirmed, Blaze retains ownership and does not
return the slot to the allocator, allowing a later destroy attempt to retry the
operation.

After a daemon restart, a later destroy request can reconstruct a recorded
network slot. Blaze does not run a background scan or retry controller for
orphaned network resources.

## Host Integration Boundary

Blaze configures the sandbox-local network path. Routing beyond the host and DNS
configuration remain the host operator's responsibility. Before enabling the
option in production, configure the required upstream routing or translation
and verify guest connectivity for the host environment.

To disable the capability, set `enable_network = false` or remove the key, then
destroy existing network-enabled sandboxes through the normal instance API.

## Guest Operations

Guest operations are available only while a sandbox is `Running` and its
backend reports a compatible guest endpoint. A cold create that reports such
an endpoint waits for the guest agent before publishing `Running`. Backends
without an endpoint, including production mock fallback, skip that wait and
return HTTP 409 for guest operations. Warm-pool activation validates the
retained backend owner and storage before publishing `Running`, but it does
not repeat the guest readiness probe. `Running` on this path therefore does
not guarantee that the guest endpoint is still responsive: the first guest
request performs the normal bounded connection and can return a guest error.
Callers should apply the retry and outcome rules below to that first request.

Guest operations and lifecycle changes use the same per-sandbox operation
lock. After obtaining the lock, the manager checks `Running` again so a request
does not contact an old runtime after a concurrent lifecycle change.

The sandbox routes are:

- `POST /v1/sandboxes/{id}/exec` — execute one command;
- `POST /v1/sandboxes/{id}/read` — read one file; and
- `POST /v1/sandboxes/{id}/write` — replace one file.

The corresponding `/v1/instances/{id}/...` routes provide the same behavior.
Exec requests use the following shape:

```json
{"cmd":"uname -a","cwd":"/","env":{"LANG":"C"},"timeout":10}
```

Write requests provide a path and standard-base64 data:

```json
{"path":"/tmp/input","data_b64":"aGVsbG8="}
```

Read requests provide only `path`. Successful file reads and command output
use standard base64. Exec timeouts range from 1 through 20 seconds. Guest
routes reject an HTTP envelope larger than 22 MiB while reading it, and file
data is limited to 16 MiB after decoding.

A failure before exec or write delivery is safe for caller-directed retry. A
pre-delivery timeout uses `"code": "guest_timeout"`. If delivery began but
the daemon cannot determine the result, it returns HTTP 504 with
`"code": "guest_outcome_unknown"`; reconcile guest state instead of
automatically replaying the operation. Reads do not change guest state.
Oversized input returns HTTP 413. An oversized read response returns HTTP 502
with `"code": "guest_response_too_large"`.

Each request is fully buffered within its per-request limit. The limit does not
bound aggregate concurrency, so clients should also cap concurrent guest
operations. Streaming files, interactive terminals, and session reuse are not
supported.

The optional TCP listener does not yet enforce a daemon-wide access boundary.
Leave `listen.http_addr` disabled in production until
[issue #2223](https://github.com/alibaba/anolisa/issues/2223) is resolved.
Daemon shutdown also does not yet wait for every active HTTP handler or release
all runtime owners, so an in-flight request may observe a closed connection.

## Storage Artifact Synchronization

Blaze can periodically persist the already-written host artifacts and directory
metadata owned by running sandboxes. The worker is disabled by default, so
existing deployments retain their previous behavior until an interval is
configured.

### Configuration

Set the interval and per-sandbox deadline in the daemon configuration:

```toml
[storage]
sync_interval = "30s"
sync_timeout = "10s"
```

`sync_interval = "disabled"` stops the periodic worker. `sync_timeout`
bounds how long the scheduler waits for one complete provider attempt:
reconstructing its storage slot and synchronizing that slot.

Each storage-provider synchronization call persists the already-written bytes
and directory metadata visible to that call. Concurrent artifact updates may
become visible in the current attempt or a later one.

### Runtime behavior

Each sweep selects sandboxes that are running and still own a complete storage
slot. A sandbox whose operation lock is already held is deferred without
waiting, allowing the sweep to continue to later sandboxes. Lifecycle changes,
guest requests, and storage artifact synchronization share this lock. After acquiring
an available lock, the worker rechecks lifecycle state before calling the
storage provider. A record that still says `Running` after the lock is acquired
but retains an unfinished operation or non-running backend ownership is
inconsistent and is reported as failed rather than deferred.
The first sweep starts after one complete configured interval. Missed timer
ticks are skipped instead of queued, preventing a slow sweep from accumulating
work.

A completed failure affects only that sandbox. Blaze retains storage ownership
and leaves lifecycle state unchanged, so a later sweep or destroy can retry.
If filesystem work cannot stop at the deadline, it keeps the sandbox operation
lock and the single synchronization permit until completion. Later attempts
are deferred instead of accumulating additional blocking work. Guest and
lifecycle operations that arrive while the lock is retained wait for the
provider work to finish; `sync_timeout` bounds scheduler waiting, not those
operations.

When the service loop stops, Blaze cancels and joins the periodic scheduler.
Provider work that cannot be cancelled remains under its sandbox lock until it
completes. Daemon-wide connection draining and runtime cleanup remain separate.

## Template Catalog

Blaze can atomically publish operator-prepared runtime artifacts and expose
their metadata through the daemon API. `/v1/templates` is the single
operator-facing template resource. Publishing an entry does not yet make
sandbox creation select or boot it.

Future sandbox-create support will resolve an optional template name from this
same catalog; there is no separate process-local registry for operators to
configure or monitor.

### Configuration

The catalog directory has a default, but imports remain disabled until an
operator configures an import root:

```toml
[template]
dir = "/var/lib/blaze/templates"
import_root = "/var/lib/blaze/template-imports"
max_files = 32
max_bytes = 274877906944
max_metadata_bytes = 1048576
max_total_bytes = 1099511627776
max_entries = 128
```

Both roots must be absolute and disjoint from each other, from Blaze image,
instance, and policy roots, from every executable path configured in
`[backends]`, from the resolved location captured when the daemon configuration
file is opened for this startup, from that file's configured pathname, and from
the configured `daemon.socket` path and the host network coordination path
`/run/lock/blaze-network.lock`. They must also remain disjoint from the
conventional named network namespace trees `/var/run/netns` and `/run/netns`.
Relative `[backends]` paths are resolved once against the daemon's startup
working directory; boundary checks, backend probing, and sandbox launch then
reuse that absolute path. When a configured backend path is a symbolic link,
both the configured link location and its resolved target remain outside
template catalog ownership.
The same rule applies when the daemon configuration path is a symbolic link:
both the configured link location and the opened file's resolved location stay
outside template catalog ownership.
Template catalog roots must not contain symbolic link components. On Linux,
Blaze compares resolved path prefixes and their underlying filesystem locations
from the mount table, so symbolic-link and bind-mounted aliases cannot bypass
these directory boundaries. Blaze retains the opened configuration file and
rechecks its identity at the captured location, so retargeting the pathname
cannot substitute another configuration file. An overlap is rejected before catalog permissions are
changed or catalog entries are scanned. A template catalog root may use a
non-UUID child of `daemon.state_dir`, as the default does, but it cannot own the
state root or enter a sandbox UUID subtree.
If the catalog root does not exist yet, Blaze retains the deepest existing
parent directory and creates the missing suffix relative to that directory.
Startup stops if any planned component appears during validation, before Blaze
changes that object's permissions. Policy-entry boundary discovery follows
`policy.on_load_error`: a discovery failure in `warn` mode uses the same empty
policy engine as policy loading, while successfully discovered policy targets
remain protected. Executable files found through `PATH` for Blaze's host helper
commands are protected as well, including both their configured and resolved
locations.
Blaze retains the validated import-root directory opened at startup. Replacing
the configured pathname later does not redirect source lookup.

### Import and lookup

Publish a source directory below `import_root`:

```http
POST /v1/templates/import
Content-Type: application/json

{"name":"runtime-base","source":"runtime-base","description":"base runtime"}
```

`source` must be relative and must not traverse parent directories or links.
The source contains top-level regular files `vmstate.snap`, `mem.bin`, and
`rootfs.ext4`; `template.json` is optional and must be a JSON object. Source
directories and files must be owned by the daemon user and not writable by
group or other users. Nested directories, links, and special files are
rejected.
Published files must have exactly one hard link, and catalog entries and staging
directories must remain on the catalog root's mount. Blaze stops rather than
changing or traversing data that violates these boundaries.
Before startup scans or list/get reads open an artifact for reading, Blaze
classifies it without a read-capable handle and rechecks the opened object's
identity. On Linux, the readable handle is derived from the pinned classified
object, so replacing the directory entry cannot redirect the read.

Use `GET /v1/templates` to list sorted name-only summaries and
`GET /v1/templates/{name}` to read one entry's complete metadata. The
daemon validates entries one at a time while listing and retains at most one
list response until its body is released; a concurrent list request receives
`503 Service Unavailable`. It separately retains at most one complete item
response; another item request receives `503 Service Unavailable` until the
first response body is released. A duplicate name or a concurrent import of
the same name returns `409 Conflict`.

### Publication, limits, and recovery

Blaze enforces the configured per-entry file and byte limits while inspecting
input. It also reserves catalog bytes and one of the `max_entries` slots before
copying into a private staging directory. It rechecks source identity after
copying, synchronizes the complete entry, and publishes it with a no-replace
rename. Readers therefore see either no entry or a complete entry. Name-only
list responses cannot materialize more than the configured number of entries.

Failed imports remove their staging data, including a staging directory whose
post-creation open or validation fails. If cleanup or publication durability
cannot be confirmed, later imports are rejected until the catalog is repaired
and the daemon restarts. Startup validates published entries and removes owned
staging directories left by an interrupted import. Before either action, the
daemon obtains and retains an exclusive lock on the opened catalog root; a
second daemon using the same catalog fails before it can inspect or clean a live
import. Graceful shutdown rejects new imports, cancels active copies, and waits
for their file handles to close.

The API validates artifact structure, not whether a snapshot can boot with a
particular backend. Sandbox create does not yet accept a template name, and the
catalog does not yet expose deletion or reference tracking.
