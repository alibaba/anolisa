# Architecture

cosh-ng uses a 5-crate Rust workspace architecture, version 0.11.0, requiring Rust 1.74+.

## Crate Dependency Graph

```
cosh-types          cosh-platform          cosh-cli / cosh-core
  (pure types)    ← (distro detection +  ← (CLI entry / Agent core)
  zero side effects   backend routing)

cosh-shell
  (independent crate, no internal dependencies)

Dependency direction: cosh-cli / cosh-core → cosh-platform → cosh-types
                     cosh-shell is independent (communicates with cosh-core process via stdin/stdout)
```

## Crate Responsibilities

| Crate | Binary | Responsibility |
|-------|--------|---------------|
| `cosh-types` | — | Pure data types, zero side effects. Defines CoshResponse envelope, CoshError, ws-ckpt IPC types |
| `cosh-platform` | — | Platform abstraction layer. Distro detection, package manager routing, systemd adapter, ws-ckpt IPC client, audit system |
| `cosh-cli` | `cosh-cli` | CLI entry. 4 command domains (pkg/svc/checkpoint/audit), JSON output |
| `cosh-core` | `cosh-core` | Agent core. Headless JSONL backend, LLM integration, hooks, tools, skills, extensions, sessions |
| `cosh-shell` | `cosh-shell` | Interactive terminal. PTY host, OSC markers, AI adapters, approval control, TUI rendering |

## Directory Layout

```
cosh-ng/
├── crates/
│   ├── cosh-types/       # Pure type definitions
│   │   └── src/          # audit.rs, checkpoint.rs, config.rs, error.rs, output.rs, pkg.rs, svc.rs
│   ├── cosh-platform/    # Platform abstraction
│   │   └── src/          # audit/, checkpoint.rs, detect.rs, pkg.rs, svc.rs, validate.rs
│   ├── cosh-cli/         # CLI binary
│   │   ├── src/          # main.rs, cmd/{pkg,svc,checkpoint,audit}.rs
│   │   └── tests/        # Integration tests
│   ├── cosh-core/        # Agent core binary
│   │   └── src/          # main.rs, core.rs, headless.rs, hook.rs, provider/, tool/, skill/, extension/
│   └── cosh-shell/       # Interactive terminal binary
│       ├── src/          # main.rs, adapter/, agent/, approval/, hooks/, shell_host/, tools/, ui/
│       └── tests/        # Layered tests
├── Cargo.toml            # Workspace configuration
└── rust-toolchain.toml
```

## Data Flow

### cosh-cli Execution Flow

```
User command → clap parsing → cmd module routing → cosh-platform backend execution → CoshResponse<T> JSON output
```

### cosh-core Headless Flow

```
stdin JSONL → message parsing → UserPromptSubmit hook → LLM generation → tool calls → approval protocol → stdout JSONL
```

### cosh-shell Interactive Flow

```
User input → PTY host → OSC boundary detection → AI adapter (launches cosh-core subprocess)
           → streaming response → approval card rendering → tool execution result display
```

## Key Design Constraints

- **ws-ckpt IPC wire format** — bincode + 4-byte little-endian length prefix. Enum variant order is the binary contract, cannot be reordered
- **Unified JSON envelope** — All cosh-cli commands return `CoshResponse<T>` (ok + data/error + meta)
- **Cross-distro routing** — `Distro::detect()` reads `/etc/os-release` to route to correct backend
- **Tool classification** — ReadOnly / FileEdit / ShellExec / ShellEvidence, approval mode decides based on this
- **Hook aliasing** — cosh-ng internal tool names map bidirectionally with copilot-shell standard names

## Dependency Management

All third-party dependencies declare versions in `[workspace.dependencies]`, sub-crates reference via `dep = { workspace = true }`. Key dependencies:

| Dependency | Purpose |
|-----------|---------|
| `serde` / `serde_json` | Serialization |
| `clap` | CLI argument parsing |
| `tokio` | Async runtime (cosh-core) |
| `reqwest` | HTTP client (LLM API) |
| `tracing` | Structured logging |
| `ratatui` | TUI rendering (cosh-shell) |
| `nix` | Unix system calls |
| `bincode` | ws-ckpt IPC serialization |
