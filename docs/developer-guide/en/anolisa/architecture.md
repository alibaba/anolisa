# Architecture

anolisa uses a 5-crate Rust workspace architecture, version 0.1.20, requiring Rust 1.88+ (edition 2024).

## Crate Dependency Graph

```
anolisa-env             anolisa-platform
  (env facts)             (platform abstractions)
      \                       /    \
       \                     /      \
        anolisa-core ---------       \
        (planning & execution)        \
              |    \                   \
              |     anolisa-build ------
              |     (build backends)
              |          |
           anolisa-cli    |
           (CLI entry) ---+

Dependency direction: anolisa-cli -> anolisa-core -> anolisa-env, anolisa-platform
                      anolisa-cli -> anolisa-build -> anolisa-core, anolisa-env, anolisa-platform
                      anolisa-cli -> anolisa-env, anolisa-platform
```

## Crate Responsibilities

| Crate | Binary | Responsibility |
|-------|--------|---------------|
| `anolisa-cli` | `anolisa` | CLI entry point. clap parser, command dispatch, response rendering. 18 leaf commands, ~60 flags |
| `anolisa-core` | — | Planning & execution engine. CLI-agnostic, 38 source modules. Handles manifest parsing, dependency resolution, install runner, lifecycle, transactions, adapter management, registry client, telemetry |
| `anolisa-env` | — | Environment facts. OS detection, kernel version probes, distro identification, framework discovery, platform capability probing |
| `anolisa-build` | — | Build backends. Currently cargo; designed for extensibility (make, npm in future) |
| `anolisa-platform` | — | Platform abstractions. Filesystem layout (FHS 3.0 user/system modes), package manager routing, RPM query/transaction, systemd integration, privilege escalation, IPC |

## Directory Layout

```
src/anolisa/
├── Cargo.toml                # Workspace config, shared dependencies
├── AGENTS.md                 # AI agent constraints for this component
├── CHANGELOG.md              # User-perceivable changes per release
├── docs/                     # Component-specific design docs
├── crates/
│   ├── anolisa-cli/          # CLI binary
│   │   └── src/
│   │       ├── main.rs       # Entry point
│   │       ├── commands.rs   # Top-level Cli struct, Commands enum, dispatch()
│   │       ├── commands/
│   │       │   ├── common.rs     # Shared helpers (layouts, state, catalog)
│   │       │   ├── adapter.rs    # AdapterArgs, AdapterCommands
│   │       │   ├── register.rs   # RegisterArgs, RegisterCommands, UnregisterArgs
│   │       │   ├── osbase.rs     # OsbaseArgs, kernel/sandbox/security subcommands
│   │       │   ├── system.rs     # SystemArgs (serve, setup, teardown, status)
│   │       │   └── tier1/
│   │       │       ├── list.rs       # ListArgs
│   │       │       ├── install.rs    # InstallArgs
│   │       │       ├── uninstall.rs  # UninstallArgs
│   │       │       ├── status.rs     # StatusArgs
│   │       │       ├── update.rs     # UpdateArgs, UpdateCommands
│   │       │       ├── doctor.rs     # DoctorArgs
│   │       │       ├── logs.rs       # LogsArgs
│   │       │       ├── restart.rs    # RestartArgs
│   │       │       ├── repair.rs     # RepairArgs
│   │       │       ├── forget.rs     # ForgetArgs
│   │       │       ├── adopt.rs      # AdoptArgs
│   │       │       ├── env.rs        # EnvArgs
│   │       │       └── bug.rs        # BugArgs
│   │       └── tests/            # Integration tests
│   ├── anolisa-core/          # Planning & execution engine
│   │   └── src/
│   │       ├── lib.rs             # Module declarations, public API
│   │       ├── manifest.rs        # TOML manifest parsing (Manifest v2)
│   │       ├── catalog.rs         # Component catalog management
│   │       ├── registry.rs        # Registry client config
│   │       ├── resolver.rs        # Dependency resolution
│   │       ├── install_runner.rs  # Install execution pipeline
│   │       ├── lifecycle.rs       # Component lifecycle state machine
│   │       ├── transaction.rs     # Crash-safe transaction journal
│   │       ├── component.rs       # Component model
│   │       ├── instance.rs        # Component instance state
│   │       ├── dependency.rs      # Dependency graph
│   │       ├── state.rs           # ANOLISA state directory
│   │       ├── backup.rs          # File backup for rollback
│   │       ├── integrity.rs       # SHA256 verification
│   │       ├── lock.rs            # File locking
│   │       ├── distribution.rs    # Distribution selectors
│   │       ├── download.rs        # Artifact download
│   │       ├── provisioner.rs     # Artifact provisioning
│   │       ├── metadata.rs        # Component metadata
│   │       ├── service.rs         # Service lifecycle
│   │       ├── hooks.rs           # Install/uninstall hooks
│   │       ├── capability.rs      # Capability declarations
│   │       ├── feature_flags.rs   # Feature flag system
│   │       ├── health.rs          # Health check module
│   │       ├── osbase_install.rs  # OS base layer installer
│   │       ├── self_update.rs     # CLI self-update logic
│   │       ├── daemon_server.rs   # Daemon mode server
│   │       ├── system_helper.rs   # System helper daemon integration
│   │       ├── central_log.rs     # Centralized logging
│   │       ├── register.rs        # Co-Build registration
│   │       ├── telemetry.rs       # Telemetry module
│   │       ├── process.rs         # Process management
│   │       ├── path_safety.rs     # Path traversal safety
│   │       ├── sandbox_manifest.rs# Sandbox scenario manifests
│   │       ├── adapter.rs         # Adapter module
│   │       ├── adapter/
│   │       │   ├── manager.rs     # Adapter lifecycle manager
│   │       │   ├── driver.rs      # Adapter driver trait
│   │       │   ├── contract.rs    # Adapter contract definition
│   │       │   ├── claim.rs       # Adapter claim (metadata)
│   │       │   ├── registry.rs    # Adapter registry
│   │       │   ├── openclaw.rs    # OpenClaw adapter
│   │       │   └── hermes.rs      # Hermes adapter
│   │       ├── health/
│   │       │   ├── engine.rs      # Health check engine
│   │       │   └── spec.rs        # Health check specification
│   │       ├── registry/
│   │       │   ├── client.rs      # HTTP registry client
│   │       │   ├── cache.rs       # Registry cache
│   │       │   ├── config.rs      # Registry configuration
│   │       │   └── error.rs       # Registry error types
│   │       └── telemetry/
│   │           ├── ilogtail.rs    # iLogtail integration
│   │           └── ops_telemetry.rs # Operations telemetry
│   ├── anolisa-env/            # Environment facts
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── cache.rs           # Probe result caching
│   │       ├── gate.rs            # Capability gating
│   │       ├── probes.rs          # Probe orchestration
│   │       └── probes/
│   │           ├── distro.rs      # Distribution detection (Alinux, Anolis, etc.)
│   │           ├── kernel.rs      # Kernel version, features (eBPF, BTF, etc.)
│   │           ├── platform.rs    # Platform details (arch, OS)
│   │           └── frameworks.rs  # Agent framework discovery
│   ├── anolisa-build/          # Build backends
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── backends.rs        # Backend registry and dispatch
│   │       └── backends/
│   │           └── cargo.rs       # Cargo build backend
│   └── anolisa-platform/       # Platform abstractions
│       └── src/
│           ├── lib.rs
│           ├── fs_layout.rs       # FHS 3.0 path layout (user/system modes)
│           ├── package_manager.rs # Package manager abstraction
│           ├── pkg_query.rs       # Package query interface
│           ├── pkg_transaction.rs # Package transaction interface
│           ├── rpm_query.rs       # RPM database query
│           ├── rpm_repo.rs        # RPM repository management
│           ├── rpm_transaction.rs # RPM install/remove transactions
│           ├── systemd.rs         # systemd unit management
│           ├── command.rs         # Command execution helpers
│           ├── privilege.rs       # Privilege detection and escalation
│           └── ipc.rs             # IPC helpers (sockets)
```

## Data Flow

### Command Dispatch Flow

```
User command → clap parsing (commands.rs Cli struct)
  → Commands enum (ComponentCommands | ManagementCommands)
  → dispatch() routing to command handler
  → anolisa-core planning/execution
  → anolisa-platform for OS-level operations
  → JSON or human-readable output
```

### Install Flow

```
anolisa install <component>
  → anolisa-core: catalog lookup → manifest parsing
  → anolisa-core: dependency resolution → install plan
  → anolisa-core: transaction journal start
  → anolisa-core: download artifact → verify sha256
  → anolisa-platform: fs_layout resolve paths
  → anolisa-platform: systemd service setup (if applicable)
  → anolisa-core: backup files → provision → hooks
  → anolisa-core: adapter scan/register (if applicable)
  → anolisa-core: transaction journal commit
```

### Sandbox Install Flow (5-Phase)

```
anolisa osbase sandbox install <target>
  → Phase 1: Pre-flight (anolisa-env probes: distro, kernel, platform)
  → Phase 2: Packages (anolisa-platform: rpm install dependencies)
  → Phase 3: OS Primitives (anolisa-core: kernel modules, eBPF programs)
  → Phase 4: Service Setup (anolisa-platform: systemd enable/start)
  → Phase 5: Post-verify (anolisa-core: health checks)
```

## Key Design Decisions

### Transaction Journal

Every destructive operation (install, uninstall) is wrapped in a crash-safe TOML journal with sha256-verified file backups. On failure, the journal provides automatic rollback. Each step records its state: pending → in_progress → completed → (rolled_back on failure).

### Manifest Schema v2

Typed TOML manifest with:
- Distribution selectors (OS, version, arch filtering)
- Artifact resolution (URL + sha256)
- Build backend declarations
- Adapter specifications
- Health check specifications
- All enforced at the type boundary via serde deserialization

### Dual-Mode Install

Two install scopes per `file-hierarchy(7)`:
- **User mode** (`~/.local/`): No root required, suitable for single-user setups
- **System mode** (`/usr/local/`): FHS 3.0 compliant, requires root; redirectable via `--prefix`

### Adapter Subsystem

Bridges ANOLISA components into agent frameworks (OpenClaw, Hermes) with:
- **Claim**: Component declares supported adapters in its manifest
- **Driver**: Framework-specific integration code
- **Contract**: Defines the interface between component and framework
- **Manager**: Lifecycle (scan, enable, disable, status)

### Crash Safety

- `transaction.rs`: TOML journal with per-step state tracking
- `backup.rs`: sha256-verified file backups before modifications
- `lock.rs`: File-based locking to prevent concurrent operations
- `integrity.rs`: Artifact and file integrity verification

## Build Profile

Release builds use:
- `lto = "thin"` for cross-crate optimization
- `codegen-units = 1` for maximum optimization
- `panic = "abort"` for smaller binary size
- `debug = true` for usable backtraces
