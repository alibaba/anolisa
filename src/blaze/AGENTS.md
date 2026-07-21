# AGENTS.md — anvil

> Common Rust conventions (comments, module layout, dependency management, error handling, pre-commit checks, commit conventions) are defined in the [root AGENTS.md](../../AGENTS.md). This file documents **anvil-specific** additions only.

## Architecture

anvil is a **daemon-only** per-host sandbox orchestrator. All sandbox management is exposed via HTTP API; the binary only handles daemon lifecycle (start / reload / doctor).

Two-crate workspace:

- **anvil-core** (library): policy engine, lifecycle state machine, backend selector, pool manager, template registry, kernel hook registry, config schema. Zero I/O beyond local TOML/JSON parsing.
- **anvil** (binary): daemon HTTP server (UDS + TCP), spawner implementations, metrics endpoint, CLI for daemon lifecycle commands.

Dependency direction: `anvil` → `anvil-core`. No reverse dependency.

## Build & Test

```bash
cd src/anvil
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

Platform: Linux (x86_64 + aarch64) for production. macOS builds succeed but spawners auto-downgrade to `MockSpawner`.

## Key Design Constraints

- **Daemon-only API model**: No CLI client for sandbox operations. All instance/pool/template management is done via HTTP endpoints on UDS (`/run/anvil/api.sock`) or TCP (`:14159`). The CLI subcommands (`daemon start`, `daemon reload`, `daemon doctor`) only manage daemon lifecycle.
- **BackendSpawner trait**: All backend-specific process management is behind `BackendSpawner`. Adding a new backend means implementing `spawn()`, `wait()`, `kill()`, `probe()` and registering it in `daemon::build_spawner()`.
- **Policy-driven backend selection**: Workload class → policy file → prioritized backend list. The daemon probes backends at startup and selects the first available. Never hardcode backend preference in application logic.
- **Lifecycle state machine**: 8 states (Pending → Creating → Running → Paused → Checkpointed → Reset → Warm → Destroyed). State transitions are enforced by `anvil_core::lifecycle`. Do not bypass via direct field mutation.
- **MockSpawner fallback**: When the configured backend binary is missing or fails `probe()`, the daemon auto-downgrades to `MockSpawner` with a warning. This keeps API/integration tests functional without a real backend.

## Adding a New Backend

1. Add a variant to `BackendKind` in `anvil-core/src/backend.rs`
2. Implement `BackendSpawner` in `anvil/src/spawner.rs`
3. Register the new spawner in `daemon::build_spawner()` priority logic
4. Add a corresponding `[backends.<name>]` section in config schema (`anvil-core/src/config.rs`)
5. Add policy support: allow the new backend kind in policy `backends` priority lists
6. Add unit tests for `probe()` and `spawn()` (use mock paths for CI)

## Configuration

Runtime config: `/etc/anolisa/anvil/config.toml` + `/etc/anolisa/anvil/policies/*.toml`

Development config: `src/anvil/examples/config.toml` + `src/anvil/examples/policies/`

When modifying config schema, update both the Rust struct in `config.rs` and the example files.

## Commit Scope

Use scope `anvil` for all changes under `src/anvil/`. Examples:

```
feat(anvil): add snapshot backend
fix(anvil): handle missing rootfs gracefully
```

## Verification

Before committing:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo doc --workspace --no-deps   # ensure no broken intra-doc links
```
