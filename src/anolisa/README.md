# ANOLISA CLI

The ANOLISA CLI (`anolisa`) is the unified management tool for [Agentic OS](https://agentic-os.sh/). It handles component lifecycle (install, update, uninstall, status, doctor), sandbox substrate provisioning, environment diagnostics, adapter management, and the Co-Build Program registration flow.

**Version**: 0.1.9 | **License**: Apache-2.0 | **Language**: Rust | **Platform**: Linux (x86_64 / aarch64)

## Features

- **Component Lifecycle** — Install, update, uninstall, and query health of ANOLISA-managed components via configured backends.
- **5-Phase Sandbox Installer** — Pre-flight → Packages → OS Primitives → Service Setup → Post-verify pipeline for 6 sandbox backends (container, kata, firecracker, gvisor, vm, landlock).
- **Transaction Journal** — Every destructive operation (install, uninstall) is wrapped in a crash-safe TOML journal with sha256-verified file backups and automatic rollback on failure.
- **Manifest Schema v2** — Typed TOML manifest with distribution selectors, artifact resolution, build backend declarations, adapter specs, and health-check spec — all enforced at the type boundary.
- **Adapter Subsystem** — Bridge ANOLISA components into agent frameworks (e.g. OpenClaw) with scan, enable, disable, and status verbs.
- **Co-Build Registration** — Opt-in usage report with consent tracking, interactive confirmation, and clean unregistration path.
- **FHS 3.0 Path Layout** — Dual-mode install: user-mode (`~/.local/` per `file-hierarchy(7)`) and system-mode (`/usr/local/` per FHS 3.0, redirectable via `--prefix`).
- **Machine-Readable Output** — `--json` flag across every command for scripting and CI integration.
- **Dry-Run Mode** — `--dry-run` prints the plan without executing, supported by install, uninstall, sandbox install, and adapter enable.

## Architecture

ANOLISA CLI is a 5-crate Rust workspace:

```
src/anolisa/
├── crates/
│   ├── anolisa-cli/      # Binary entry point — clap parser, command dispatch, response rendering
│   ├── anolisa-core/      # Planning & execution engine — CLI-agnostic, 38 modules
│   ├── anolisa-env/       # Environment facts — OS detection, kernel version, package manager
│   ├── anolisa-build/     # Build backends — cargo, make, future npm
│   └── anolisa-platform/  # Platform abstractions — FsLayout, package manager, privilege
└── manifests/             # Component manifest TOML files
```

### Crate Responsibilities

| Crate | Role | Key Modules |
|-------|------|-------------|
| `anolisa-cli` | CLI surface: clap parsing, global flags → `CliContext`, command dispatch, error rendering, color | `commands.rs` (two-tier dispatch), `context.rs`, `response.rs`, `repo_config.rs` |
| `anolisa-core` | Planning & execution engine: lifecycle planning, transaction journal, manifest schema, component metadata, sandbox installer, adapter manager, registration | `lifecycle.rs`, `transaction.rs`, `manifest.rs`, `component.rs`, `sandbox_install.rs`, `adapter/`, `register.rs` |
| `anolisa-env` | Environment fact-gathering: OS detection, kernel version, CPU features, package manager presence | `lib.rs` (`EnvFacts`, `EnvService`) |
| `anolisa-build` | Build backend abstraction: cargo build executor, future make/npm backends | `backends/cargo.rs` |
| `anolisa-platform` | OS-level abstractions: filesystem layout (FHS 3.0), package manager detection, privilege ops | `fs_layout.rs`, `package_manager.rs`, `privilege.rs` |

### Command Dispatch

```
anolisa                      # "ANOLISA — Agentic OS helper"
├── Component Commands       # Everyday component lifecycle
│   ├── list, ls            # List available components from remote catalog
│   ├── install             # Install a component (raw/rpm/npm backends)
│   ├── uninstall           # Remove a component (with rollback journal)
│   ├── status              # Show installed component health
│   ├── doctor              # Diagnose and optionally fix issues
│   ├── logs                # Query centralized log
│   ├── restart             # Restart a component's service
│   ├── update              # Update CLI or runtime components
│   ├── repair              # Reconcile state with rpmdb after manual RPM changes
│   ├── forget              # Drop ANOLISA state record (escape hatch)
│   ├── adopt               # Track already-installed system RPM
│   └── adapter             # Manage component-to-framework adapters
└── Management Commands     # System-level administration (root)
    ├── register            # Join Agentic OS Co-Build Program
    ├── unregister          # Leave Co-Build Program
    ├── env                 # Show environment detection results
    ├── bug                 # Generate a bug report
    ├── system              # System helper daemon management
    │   ├── setup           # Install system helper daemon
    │   ├── serve           # Start daemon in foreground
    │   ├── status          # Check daemon health
    │   └── teardown        # Remove daemon
    └── osbase              # OS base layer management
        ├── sandbox         # Install/remove/list/status sandbox backends
        ├── kernel          # Kernel module/eBPF management (NOT_IMPLEMENTED)
        └── security        # Security overlay management (NOT_IMPLEMENTED)
```

## Quick Start

### Prerequisites

- Linux (x86_64 or aarch64)
- Rust ≥ 1.88

### Build from Source

```bash
# From the anolisa repo root
cd src/anolisa
cargo build --release

# The binary is at target/release/anolisa
```

Or use the unified build script:

```bash
./scripts/build-all.sh --component cli    # TODO: CLI component selector
```

### Basic Usage

```bash
# See all available components
anolisa list

# Install a component
anolisa install copilot-shell

# Check component health
anolisa status
anolisa status copilot-shell

# Diagnose issues
anolisa doctor

# Update the CLI itself
anolisa update self

# Update all components
anolisa update all

# Machine-readable output
anolisa status --json
anolisa list --json
```

### Global Flags

| Flag | Description |
|------|-------------|
| `--install-mode <user\|system>` | Install scope: user (`~/.local`) or system (`/usr/local`). Default: `user`. |
| `--prefix <PATH>` | Custom install prefix (system-mode only). Must be absolute, no `..` segments. |
| `--json` | Output in machine-readable JSON format. |
| `--dry-run` | Print the execution plan without making changes. |
| `-v, --verbose` | Increase verbosity. |
| `-q, --quiet` | Suppress non-error output. |
| `--no-color` | Disable colored output. |

## Component Commands

### `list` (alias: `ls`)

List available components from the remote catalog.

```bash
anolisa list
anolisa ls
anolisa ls
```

Reads the catalog from the configured repository and displays installed/available components with version and layer information.

### `install`

Install a component from a configured backend. Supports `raw` (direct download with sha256 verification), with `yum` and `npm` backends planned.

```bash
anolisa install <COMPONENT>
anolisa install <COMPONENT> --version 1.2.3
anolisa install <COMPONENT> --backend raw
anolisa install --all                    # Install everything in the catalog
anolisa install --all --fail-fast        # Stop on first failure
```

**Backends**: `raw` (direct download with sha256 verification) and `rpm` (system package). `npm` is planned.

**Resolution chain**: `--backend` > `default_backend` in `repo.toml` → component name → package map → raw artifact URL.

Every `install` opens an `InstallLock` (filesystem lock for race-free state mutation), downloads the artifact with mandatory sha256 verification, loads the install contract from the embedded component manifest, installs declared files, records state in `installed.toml`, and writes a central-log audit entry. The operation is wrapped in a crash-safe `Transaction` journal.

### `uninstall`

Remove a component. Only removes files marked `FileOwner::Anolisa`; external files are preserved.

```bash
anolisa uninstall <COMPONENT>
anolisa uninstall <COMPONENT> --dry-run
```

Uses the `LifecyclePlan` data model: snapshots state, builds a plan (what's owned, what's safe to remove, what's external), and executes inside a `Transaction` journal. On failure, the journal is walked backwards calling rollback primitives.

### `status`

Read-only view of installed components.

```bash
anolisa status                    # All installed components
anolisa status <COMPONENT>        # One component
anolisa status --json             # Machine-readable
```

Reports: component name, version, install time, object status, service state, integrity summary, adapter claim status, and health check results.

### `doctor`

Diagnose and optionally fix component issues.

```bash
anolisa doctor                    # All installed components
anolisa doctor <COMPONENT>        # One component
anolisa doctor --fix              # Apply fixes (inside a transaction)
```

`--dry-run --fix` is rejected: `--dry-run` alone prints the diagnostic plan; `--fix` is the explicit "execute" verb.

### `logs`

Query the centralized log, optionally filtered by component and severity.

```bash
anolisa logs
anolisa logs <COMPONENT>
anolisa logs --severity warn
```

### `restart`

Restart a component's systemd service.

```bash
anolisa restart <COMPONENT>
```

### `update`

Three subcommands:

```bash
anolisa update self                      # Update CLI binary only
anolisa update runtime <COMP|all>        # Update runtime components
anolisa update all                       # Update everything (NOT self)
```

**Invariant**: `update all` does NOT include CLI self-update. The binary swap never shares a transaction with component updates.

### `repair`

Reconcile ANOLISA state with rpmdb after manual `dnf update`/`downgrade` outside ANOLISA. Reads rpmdb, confirms package identity is valid, and refreshes the ANOLISA state record — no dnf/rpm transaction.

```bash
anolisa repair <COMPONENT>
```

A package that has been `rpm -e`'d cannot be repaired — use `forget` instead.

### `forget`

Drop a component's ANOLISA state record without touching the underlying package or files. The escape hatch for stale state after manual `rpm -e`.

```bash
anolisa forget <COMPONENT>
```

### `adopt`

Record an already-installed system RPM as ANOLISA-tracked state. The explicit counterpart to `install`'s implicit system-mode adoption.

```bash
anolisa adopt <COMPONENT>     # Requires --install-mode=system
```

### `adapter`

Manage component-to-framework adapters (e.g. registering `tokenless` as an OpenClaw plugin).

```bash
anolisa adapter scan                       # Discover available adapters
anolisa adapter enable <COMP> [FRAMEWORK]  # Enable an adapter
anolisa adapter disable <COMP> [FRAMEWORK] # Disable (idempotent)
anolisa adapter status [COMP]              # Report receipt health
```

### `system`

Manage the system-helper daemon lifecycle.

```bash
anolisa system setup      # Install the system helper daemon
anolisa system serve       # Start daemon in foreground
anolisa system status      # Check daemon health
anolisa system teardown    # Remove daemon: stop service, delete unit + binary
```

## Management Commands

### `register` / `unregister`

Join or leave the Agentic OS Co-Build Program. Requires root.

```bash
sudo anolisa register              # Interactive consent flow
sudo anolisa register --yes        # Skip confirmation (for scripts)
anolisa register status            # Show registration status
anolisa register status --json     # Machine-readable

sudo anolisa unregister            # Opt out
sudo anolisa unregister --force    # Skip confirmation
```

The registration flow:
1. Checks existing state (already registered? via sysom?)
2. Shows a consent banner explaining what data is collected
3. Prompts for confirmation (or skips with `--yes`)
4. Starts the usage report upload service
5. Writes consent state to disk
6. `unregister` reverses: writes "unregistered" consent FIRST, then best-effort teardown of upload infrastructure

### `env`

Show environment detection results (OS, kernel, package manager, available features).

```bash
anolisa env
```

### `bug`

Generate a bug report with system diagnostics.

```bash
anolisa bug
```

### `osbase`

OS base layer management with three sub-surfaces:

```bash
# Sandbox (implemented)
anolisa osbase sandbox install <TARGET>       # Install sandbox backend
anolisa osbase sandbox install container --variant runc
anolisa osbase sandbox install firecracker --variant e2b
anolisa osbase sandbox list --available        # List backends
anolisa osbase sandbox status [TARGET]         # Status summary

# Kernel and Security (NOT_IMPLEMENTED)
anolisa osbase kernel install
anolisa osbase security install loongshield
```

**Sandbox backends**: `container` (runc/rund), `kata` (KVM-based VM), `firecracker` (microVM with standard/e2b/kata-fc variants), `gvisor` (user-space kernel, requires `--runtime`), `vm` (QEMU/KVM), `landlock` (LSM access control).

**5-phase install pipeline**: Pre-flight → Packages → OS Primitives → Service Setup → Post-verify.

Sandbox install requires `--install-mode=system` and root privileges.

## Configuration

### `repo.toml`

The repository configuration file (`<etc_dir>/repo.toml`) defines available backends and their base URLs:

```toml
default_backend = "raw"

[backends.raw]
base_url = "https://anolisa.oss-cn-hangzhou.aliyuncs.com/anolisa-releases/anolisa/v1"
package_map = { linux-sandbox = "agent-sec-core" }

[backends.yum]
base_url = "https://anolisa.oss-cn-hangzhou.aliyuncs.com/anolisa-rpms/anolisa/v1"

[backends.npm]
# Planned
```

When `repo.toml` is absent, `anolisa update` downloads the bootstrap copy from the OSS production endpoint.

### FHS 3.0 Path Layout

| Mode | Prefix | Config | Data | State | Cache |
|------|--------|--------|------|-------|-------|
| `user` | `~/.local` | `~/.config/anolisa/` | `~/.local/share/anolisa/` | `~/.local/state/anolisa/` | `~/.cache/anolisa/` |
| `system` | `/usr/local` | `/etc/anolisa/` | `/usr/local/share/anolisa/` | `/var/lib/anolisa/` | `/var/cache/anolisa/` |
| `system` with `--prefix /opt/anolisa` | `/opt/anolisa` | `/etc/anolisa/` | `/opt/anolisa/share/anolisa/` | `/var/lib/anolisa/` | `/var/cache/anolisa/` |

Config and state directories remain anchored to FHS paths even with `--prefix` — only `bin`, `lib`, and `share` are relocated.

## Transaction Journal

Every destructive lifecycle operation (install, uninstall, purge) is wrapped in a crash-safe `Transaction`:

1. **Begin** — Mints a sortable `operation_id`, snapshots the existing `installed.toml`, writes empty journal.
2. **Plan** — Each side effect is recorded as a `TransactionStep` with `Planned` status. Journal is rewritten atomically (tmp → rename).
3. **Execute** — Each step is performed; on success, status flips to `Done`.
4. **Rollback** — On failure, journal is walked backwards: restore backed-up files (sha256-verified), restore state snapshot.
5. **Repair** — After a crash, `Transaction::load_journal` reads the journal back so a future `repair` command can finish or rewind.

Backups are stored as `<state>/backups/<operation_id>/<idx>.bak` with sha256 hashes.

## Manifest Schema v2

Component manifests (`src/anolisa/manifests/*.toml`) use a typed schema v2:

- `component.meta` — name, version, layer (osbase/runtime/encapsulation), domain (tools/state/cost/security/observability)
- `component.contract` — schema version envelope
- `component.artifact` — artifact type, name, target OS/arch
- `distribution_selectors` — per-platform preferred artifact type ordering
- `build` — backend declaration (cargo, future make/npm)
- `install` — files, services, capabilities
- `dependencies` — build, runtime, and component dependency lists
- `adapters` — framework adapter declarations
- `health_check` / `health_checks` — verification specs
- `features` — feature toggle definitions

## Error Model

All CLI errors are classified into four categories for consistent exit codes and JSON output:

| Error Kind | Exit Code | When |
|------------|-----------|------|
| `InvalidArgument` | 2 | Bad user input (unknown component, invalid flag combination, missing dependency) |
| `Runtime` | 1 | Execution failure (download error, filesystem error, lock contention) |
| `Degraded` | 2 | Operation succeeded partially (install with warnings, cleanup incomplete) |
| `NotImplemented` | 1 | Feature not yet shipped (kernel/security osbase, yum/npm backends) |

## License

Apache-2.0. See [LICENSE](../../LICENSE) for details.
