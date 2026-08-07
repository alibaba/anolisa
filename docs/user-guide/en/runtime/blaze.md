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
