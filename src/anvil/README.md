# ANOLISA Anvil

Per-host sandbox daemon that manages sandbox instance lifecycles via HTTP API.

## Overview

Anvil is an API-only daemon that allocates, monitors, and destroys
sandboxed execution environments on a single host. It supports multiple backends
(Firecracker microVM, bubblewrap/bwrap) with policy-driven selection, and is
designed to be called by upper-layer platforms such as Substrate or E2B-style
orchestrators.

## Features

- **HTTP API** — Unix domain socket (`/run/anvil/api.sock`) + TCP (`:14159`)
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
cd src/anvil
cargo build --release

# Run daemon (dev: override policy.dir to use local examples)
sudo ./target/release/anvil daemon start --config examples/config.toml
# Note: the default config sets policy.dir = /etc/anolisa/anvil/policies.
# For source-checkout testing, create a symlink or override:
#   sudo mkdir -p /etc/anolisa/anvil
#   sudo ln -s $(pwd)/examples/policies /etc/anolisa/anvil/policies

# Health check
curl --unix-socket /run/anvil/api.sock http://localhost/v1/health

# Create a sandbox
curl -X POST --unix-socket /run/anvil/api.sock http://localhost/v1/instances \
  -H 'Content-Type: application/json' \
  -d '{"workload_class":"agent-rl","image_digest":"sha256:..."}'
```

## Configuration

The daemon reads a TOML config file (default: `/etc/anolisa/anvil/config.toml`)
and a policies directory containing per-workload-class policy files.

```
/etc/anolisa/anvil/
├── config.toml
└── policies/
    ├── agent-rl.toml
    └── agent-tool.toml
```

See `src/anvil/examples/` for annotated sample configurations.

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
src/anvil/
├── crates/
│   ├── anvil-core/   # Library: policy, lifecycle, pool, template, kernel, config
│   └── anvil/        # Binary: daemon, API server, spawners, metrics
├── examples/         # config.toml, policies/
├── dist/             # anvil.service, anvil.spec, tmpfiles
└── manifests/        # Component metadata
```

## Requirements

- Rust 1.88+ (see `src/anvil/rust-toolchain.toml`)
- Linux host with root privileges for sandbox backends

## License

