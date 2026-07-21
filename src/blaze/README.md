# ANOLISA Blaze

Per-host sandbox daemon that manages sandbox instance lifecycles via HTTP API.

## Overview

Blaze is an API-only daemon that allocates, monitors, and destroys
sandboxed execution environments on a single host. It supports multiple backends
(Firecracker microVM, bubblewrap/bwrap) with policy-driven selection, and is
designed to be called by upper-layer platforms such as Substrate or E2B-style
orchestrators.

## Features

- **HTTP API** — Unix domain socket (`/run/blaze/api.sock`) + TCP (`:14159`)
- **Policy-driven backend selection** — workload class → backend priority list
- **Lifecycle state machine** — 8 states (Pending → Creating → Running → Paused → Checkpointed → Reset → Warm → Destroyed)
- **Warm pool management** — pre-warmed instances with TTL-based GC
- **Template registry** — in-memory template tracking with idle eviction
- **Kernel hook registry** — state tracking for pre/post hooks
- **Prometheus metrics** — request counts, instance gauges, pool sizes
- **Spawners** — FirecrackerSpawner, BubblewrapSpawner, MockSpawner

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
curl -X POST --unix-socket /run/blaze/api.sock http://localhost/v1/instances \
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
```

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/health` | Health check |
| GET | `/v1/instances` | List all instances |
| POST | `/v1/instances` | Create a new sandbox instance |
| GET | `/v1/instances/{id}` | Get instance details |
| POST | `/v1/instances/{id}/checkpoint` | Checkpoint an instance |
| POST | `/v1/instances/{id}/reset` | Reset instance to checkpoint |
| POST | `/v1/instances/{id}/destroy` | Destroy an instance |
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

## License

