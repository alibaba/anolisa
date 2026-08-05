# ANOLISA Blaze

[中文版](README_zh.md)

Per-host sandbox orchestrator daemon for AI Agent workloads.

Blaze manages sandbox instance lifecycles via HTTP API with policy-driven
backend selection. It supports warm-pool pre-allocation, multi-backend
fallback (Firecracker → Bubblewrap → Mock), and Prometheus metrics export.
Designed as the per-host agent for E2B-style orchestrator platforms.

## Features

- **HTTP API** — Unix domain socket (`/run/blaze/api.sock`) + TCP (`:14159`)
- **Policy-driven backend selection** — workload class → backend priority list
- **Lifecycle state machine** — 9 states: Pending, Creating, Running, Paused,
  Checkpointed, RecoveryRequired, Reset, Warm, and Destroyed
- **Warm pool management** — pre-warmed instances with TTL-based GC
- **Template registry** — in-memory template tracking with idle eviction
- **Kernel hook registry** — state tracking for pre/post hooks
- **Prometheus metrics** — request counts, instance gauges, pool sizes
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
  -d '{"workload_class":"agent-rl","image_digest":"sha256:..."}'
```

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
# pool_size = 0           # [Reserved] Warm pool slots (not yet active)
# prefork = false         # [Reserved] Pre-start VMs in pool (not yet active)
# flush_interval = "30s"  # [Reserved] Dirty data flush period (not yet active)
```

The `file` provider uses standard filesystem operations for sandbox storage. The `auto` provider probes available backends in priority order (currently equivalent to `file`). Unrecognized values will log a warning and fall back to `file`.

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/health` | Health check |
| GET | `/v1/sandboxes` | List all sandboxes |
| POST | `/v1/sandboxes` | Create a sandbox |
| GET | `/v1/sandboxes/{id}` | Get sandbox details |
| DELETE | `/v1/sandboxes/{id}` | Destroy a sandbox |
| GET | `/v1/instances` | Alias for listing sandboxes |
| POST | `/v1/instances` | Alias for creating a sandbox |
| GET | `/v1/instances/{id}` | Alias for sandbox details |
| DELETE | `/v1/instances/{id}` | Alias for destroying a sandbox |
| POST | `/v1/instances/{id}/destroy` | Compatible destroy action |
| POST | `/v1/instances/{id}/checkpoint` | Record checkpoint state |
| POST | `/v1/instances/{id}/reset` | Record reset and return to the warm pool |
| GET | `/v1/pools` | List warm pools |
| GET | `/v1/pools/{backend}/{class}` | Get pool status |
| POST | `/v1/pools/{backend}/{class}/drain` | Drain a pool |
| PUT | `/v1/pools/{backend}/{class}/sizing` | Resize a pool |
| GET | `/v1/templates` | List templates |
| GET | `/v1/templates/{id}` | Inspect a template |
| POST | `/v1/templates/gc` | Trigger template GC |
| GET | `/v1/policies` | List loaded policies |
| GET | `/v1/hooks` | List kernel hooks |
| GET | `/v1/metrics` | Prometheus metrics |
| POST | `/v1/admin/reload` | Hot-reload policies |

### Managed lifecycle and recovery

Create and destroy record their operation before changing storage or backend
resources. A successful create finishes in `Running`; a successful destroy
finishes in `Destroyed`. If compensation cannot release every owned resource,
the sandbox remains visible as `RecoveryRequired` so destroy can be retried.

At startup, the daemon reconciles each non-terminal sandbox independently.
Failure to clean up one sandbox does not prevent the remaining records from
being processed or the API from starting.

The operation journal records the operation and start time, not completion of
each resource step. An interrupted create is cleaned up rather than resumed,
and an existing backend process is not adopted after restart. Failed recovery
does not run in a background retry loop. The checkpoint and reset endpoints
retain their existing metadata transitions; this recovery flow does not add
backend snapshot or restore operations.

#### Health Check

`GET /v1/health` returns daemon status including storage pool readiness:

```json
{
  "status": "ok",
  "version": "0.3.0",
  "storage_pool": { "ready": 0, "capacity": 0, "pending": 0 }
}
```

## Project Layout

```
src/blaze/
├── crates/
│   ├── blaze-core/   # Library: policy, lifecycle, pool, template, kernel, config
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
