# ANOLISA Blaze

[中文版](README_zh.md)

Per-host sandbox orchestrator daemon for AI Agent workloads.

Blaze manages sandbox instance lifecycles via HTTP API with policy-driven
backend selection. It supports multi-backend fallback
(Firecracker → Bubblewrap → Mock) and Prometheus metrics export.
Designed as the per-host agent for E2B-style orchestrator platforms.

## Features

- **HTTP API** — Unix domain socket (`/run/blaze/api.sock`) + TCP (`:14159`)
- **Policy-driven backend selection** — workload class → backend priority list
- **Lifecycle state machine** — durable state with restart recovery
- **Guest operations** — bounded command execution and file transfer for
  running backends that expose a guest endpoint
- **Template catalog** — bounded import and atomic publication of reusable artifacts
- **Kernel hook registry** — state tracking for pre/post hooks
- **Prometheus metrics** — request and instance counters
- **Spawners** — FirecrackerSpawner, BubblewrapSpawner, MockSpawner
- **Optional VM networking** — isolated namespace, tap, veth, and NAT per Firecracker VM

## Quick Start

```bash
# Build
cd src/blaze
cargo build --release

# Run daemon (dev: override policy.dir to use local examples)
sudo ./target/release/blazed daemon start --config examples/config.toml
# Note: the default config sets policy.dir = /etc/anolisa/blaze/policies.
# For source-checkout testing, create a symlink or override:
#   sudo mkdir -p /etc/anolisa/blaze
#   sudo ln -s $(pwd)/examples/policies /etc/anolisa/blaze/policies

# Health check
curl --unix-socket /run/blaze/api.sock http://localhost/v1/health

# Create a sandbox
curl -X POST --unix-socket /run/blaze/api.sock http://localhost/v1/sandboxes \
  -H 'Content-Type: application/json' \
  -d '{"workload_class":"agent-tool","image_digest":"sha256:..."}'
```

The quick-start request uses an example policy with Firecracker guest transport
disabled, so an image without the compatible guest agent does not wait for guest
readiness. Enable the transport only for images that run that agent.

## Configuration

The daemon reads a TOML config file (default: `/etc/anolisa/blaze/config.toml`)
and a policies directory containing per-workload-class policy files.

```
/etc/anolisa/blaze/
├── config.toml
└── policies/
    ├── agent-rl.toml
    └── agent-tool.toml
```

See `src/blaze/examples/` for annotated sample configurations.

### VM Resource Configuration

Blaze resolves vCPU and memory settings using a three-layer fallback chain:

1. **Backend-specific** (`[backend.firecracker].vcpus` / `.memory`) — highest priority
2. **Policy-level** (`[vm].vcpus` / `[vm].memory`) — shared across backends
3. **Code default** (1 vCPU, 256 MiB) — fallback when unspecified

Example in a policy file:

```toml
[vm]
vcpus = 2
memory = "512Mi"

[backend.firecracker]
vcpus = 4        # overrides [vm].vcpus for Firecracker only
memory = "1Gi"   # overrides [vm].memory for Firecracker only
enable_network = false
```

Set `enable_network = true` to create an isolated network slot for each
Firecracker VM. Explicit sandbox destroy and compensated startup failure remove
the namespace, tap, and veth after process termination. A destroy retried after
a daemon restart can reconstruct the recorded slot; there is no background
cleanup scan. Slot creation and deletion use a host-wide lock so independent
daemon processes cannot allocate the same host device names concurrently.
When a loaded Firecracker policy enables this option, backend probing also
checks the required commands and host privileges. The checks are skipped when
networking is disabled. Upstream routing and DNS remain host operator
responsibilities.

### Storage Configuration

The `[storage]` section controls the sandbox storage backend:

```toml
[storage]
provider = "file"       # Storage provider selection. Currently supported: "file", "auto".
                        # "auto" probes available providers in priority order (currently equivalent to "file").
                        # Other values will log a warning and fall back to file.
images_dir = "/var/lib/blaze/images"
sync_interval = "disabled" # Set a positive duration to persist already-written slot artifacts.
sync_timeout = "30s"       # Maximum scheduler wait for reconstruction plus artifact sync.
```

Reusable-instance settings are not supported. Blaze rejects
`storage.pool_size`, `storage.prefork`, and every `[pool]` section except the
exact historical package defaults. Blaze temporarily accepts and ignores those
defaults from older `config.toml`,
`agent-rl.toml`, and `agent-tool.toml` files, and logs a warning. This lets an
administrator-modified file retained by RPM `%config(noreplace)` reach the new
daemon without enabling an incomplete feature. Merge the corresponding
`.rpmnew` file or remove the old `[pool]` section; later releases may remove
this exception. Any other policy `[pool]` section fails policy loading. At
startup, `policy.on_load_error = "fail"` stops the daemon, while `"warn"` starts
with an empty policy set. A failed administrative or signal-driven reload
keeps the currently active policies unchanged.

The `file` provider uses standard filesystem operations for sandbox storage. The `auto` provider probes available backends in priority order (currently equivalent to `file`). Unrecognized values will log a warning and fall back to `file`.
When periodic synchronization is enabled, a completed provider failure is
isolated from later sandboxes. If a provider cannot stop its filesystem work at
the deadline, that work keeps the sandbox operation lock and the single
synchronization permit until completion; later attempts are deferred instead
of accumulating. The worker stops scheduling new work when the service loop
ends.

See the [Storage Artifact Synchronization user guide](../../docs/user-guide/en/runtime/blaze.md#storage-artifact-synchronization)
for configuration, selection, retry, and worker shutdown behavior.

## API Endpoints

Blaze exposes sandbox lifecycle and guest operations through `/v1/sandboxes`.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/health` | Health check |
| GET | `/v1/sandboxes` | List all sandboxes |
| POST | `/v1/sandboxes` | Create a sandbox |
| GET | `/v1/sandboxes/{id}` | Get sandbox details |
| DELETE | `/v1/sandboxes/{id}` | Destroy a sandbox |
| POST | `/v1/sandboxes/{id}/exec` | Execute a guest command |
| POST | `/v1/sandboxes/{id}/read` | Read a guest file |
| POST | `/v1/sandboxes/{id}/write` | Replace a guest file |
| GET | `/v1/pools` | Reserved; returns `501` |
| GET | `/v1/pools/{backend}/{class}` | Reserved; returns `501` |
| POST | `/v1/pools/{backend}/{class}/drain` | Reserved; returns `501` |
| PUT | `/v1/pools/{backend}/{class}/sizing` | Reserved; returns `501` |
| GET | `/v1/templates` | List published template names |
| GET | `/v1/templates/{name}` | Inspect published template metadata |
| POST | `/v1/templates/import` | Publish a template from the configured import root |
| GET | `/v1/policies` | List loaded policies |
| GET | `/v1/hooks` | List kernel hooks |
| GET | `/v1/metrics` | Prometheus metrics |
| POST | `/v1/admin/reload` | Hot-reload policies |

Upgrade compatibility accepts and ignores only this exact daemon section:

```toml
[pool]
default_warm_ttl = "30m"
gc_interval = "5m"
```

An accepted policy section must contain exactly these six fields and belong to
one of the two packaged policy identities:

| Policy name | Workload class | `min` | `target` | `max` |
|---|---|---:|---:|---:|
| `agent-rl-default` | `agent-rl` | 4 | 16 | 64 |
| `agent-tool-default` | `agent-tool` | 2 | 8 | 32 |

Both rows require `enabled = true`, `warm_ttl = "30m"`, and
`reset_mode = "full-recreate"`. A missing or additional field, a changed value
or type, a different policy name or workload class, any other `[pool]` section,
and every `storage.pool_size` or `storage.prefork` setting are rejected. The
accepted values do not enable reusable instances and are omitted when the
configuration is serialized.

Blaze continues to decode persisted `Reset`, `Warm`, and
`start_path = "warm"` values written by earlier releases. Startup
reconciliation treats non-terminal records containing those values as cleanup
candidates and never reuses them. A failed cleanup retains the in-memory record
as `RecoveryRequired` and attempts to persist that state. If persistence also
fails, the startup warning includes the additional error and the durable record
may still contain its previous state. Reconciliation continues with other
accepted records.

The `/v1/templates` routes are the single operator-facing template catalog.
Importing an entry does not yet make sandbox creation select it; future create
support will resolve optional names from this same catalog. See the
[template catalog user guide](../../docs/user-guide/en/runtime/blaze.md#template-catalog)
for configuration, accepted artifacts, limits, and publication rules.

### Managed lifecycle and recovery

Create and destroy record their operation before changing storage or backend
resources. A successful create finishes in `Running`; a successful destroy
finishes in `Destroyed`. If compensation cannot release every owned resource,
the sandbox remains visible as `RecoveryRequired` so destroy can be retried.

At startup, Blaze first validates the complete lifecycle inventory. Only after
that inventory is complete and consistent does the daemon reconcile each
non-terminal sandbox independently. A cleanup failure during this later
reconciliation leaves that sandbox `RecoveryRequired`, but does not prevent the
remaining accepted records from being processed or the API from starting.

Inventory validation is fail-closed. Startup stops before the API listeners
open if a UUID-owned entry is not a canonically named directory; if its
`state.json` is missing, unreadable, a
symbolic link or directory, or has another hard link; if the stored sandbox ID
differs from the directory name; or if a `Destroyed` record still reports an
active operation or backend ownership that may still be live. Blaze does not
repair or delete these records automatically. Before accepting the inventory,
Blaze completes a second enumeration of the canonical UUID names and compares
the complete set with the initial scan. It then checks that every retained UUID
directory and `state.json` still refer to the objects it read. Startup stops if
the second enumeration finds an added or removed entry, or if the following
object checks find that either retained object disappeared or was replaced.
This consistency contract applies to Blaze state writers: the production store
holds the state-root advisory lock, and the scan holds the in-process ownership
map lock until publication. Direct file changes by a process that bypasses the
state-root lock are unsupported.

See the
[lifecycle state consistency and compatibility design](docs/design/lifecycle-state-consistency.md)
for writer coordination, inventory publication, reset rejection, legacy-state
cleanup, and failure boundaries.

The operation journal records the operation and start time, not completion of
each resource step. An interrupted create is cleaned up rather than resumed,
and an existing backend process is not adopted after restart. Failed recovery
does not run in a background retry loop. Checkpoint capture and restore are not
available in this change. Reset remains unavailable until runtime and storage
can be reset together; this recovery flow does not add backend snapshot,
capture, or restore operations.

### Guest operations

Running sandboxes can execute bounded commands and transfer bounded files when
their backend exposes a compatible guest endpoint. Production mock fallback
does not advertise this capability. See the
[Blaze user guide](../../docs/user-guide/en/runtime/blaze.md#guest-operations)
for request formats, limits, readiness, error handling, and current shutdown
boundaries.

#### Health Check

`GET /v1/health` returns daemon status including storage capacity:

```json
{
  "status": "ok",
  "version": "0.3.0",
  "storage_pool": { "ready": 0, "capacity": 0, "pending": 0, "quarantined": 0 }
}
```

## Project Layout

```
src/blaze/
├── crates/
│   ├── blaze-core/   # Library: policy, lifecycle, template, kernel, config
│   └── blazed/       # Binary: daemon, API server, spawners, metrics
├── examples/         # config.toml, policies/
├── dist/             # blazed.service, blaze.spec, tmpfiles
└── manifests/        # Component metadata
```

## Requirements

- Rust 1.88+ (see `src/blaze/rust-toolchain.toml`)
- Linux host with root privileges for sandbox backends
- `ip`, `iptables`, `sysctl`, and network namespace privileges when VM
  networking is enabled

## License
