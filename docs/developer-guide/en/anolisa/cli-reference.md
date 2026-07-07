# CLI Reference

Complete reference of all `anolisa` CLI commands, subcommands, arguments, and flags.

---

## Global Options

These flags are available on every command (`global = true`):

| Flag | Type | Description |
|------|------|-------------|
| `--install-mode <MODE>` | `user` \| `system` | Install scope. Defaults to `system` if root, `user` otherwise |
| `--prefix <PATH>` | Path | Custom install prefix (system-mode only) |
| `--json` | Flag | Output in JSON format |
| `--dry-run` | Flag | Print plan without executing |
| `-v`, `--verbose` | Flag | Increase verbosity |
| `-q`, `--quiet` | Flag | Suppress non-error output |
| `--no-color` | Flag | Disable colored output |

---

## Component Commands

### `list` (alias: `ls`)

List available components.

```
anolisa list [--installed]
```

| Arg | Type | Description |
|-----|------|-------------|
| `--installed`, `--enabled` | Flag | Show only currently installed components |

### `install`

Install a component.

```
anolisa install <COMPONENT> [options]
```

| Arg | Type | Description |
|-----|------|-------------|
| `<COMPONENT>` | String | Component name to install |
| `--all` | Flag | Install all available components |
| `--package <NAME>` | String | Pin to a specific package name |
| `--version <VER>` | String | Pin to a specific version |
| `--force` | Flag | Force reinstall |
| `--no-verify` | Flag | Skip post-install verification |

### `uninstall`

Remove a component.

```
anolisa uninstall <COMPONENT> [--purge] [--remove-system-package] [--force]
```

| Arg | Type | Description |
|-----|------|-------------|
| `<COMPONENT>` | String | Component to uninstall |
| `--purge` | Flag | Also remove ANOLISA-owned config/cache/state fragments |
| `--remove-system-package` | Flag | For rpm-observed: delegate removal to `dnf remove` |
| `--force` | Flag | Reserved for forcing through warnings |

### `status`

Show installation status.

```
anolisa status [COMPONENT]
```

| Arg | Type | Description |
|-----|------|-------------|
| `[COMPONENT]` | String (optional) | Show detail for specific component; omit for aggregate view |

### `update`

Update installed components.

```
anolisa update [COMPONENT]
anolisa update self
anolisa update all
```

| Arg | Type | Description |
|-----|------|-------------|
| `[COMPONENT]` | String (optional) | Component to update |

Subcommands:

| Subcommand | Description |
|------------|-------------|
| `self` | Update the anolisa CLI binary only |
| `all` | Update every ANOLISA-managed object |

### `doctor`

Run health checks.

```
anolisa doctor [COMPONENT] [--fix]
```

| Arg | Type | Description |
|-----|------|-------------|
| `[COMPONENT]` | String (optional) | Diagnose a specific component; default: all installed |
| `--fix` | Flag | Apply suggested fixes automatically |

### `logs`

View operation logs.

```
anolisa logs [OBJECT] [options]
```

| Arg | Type | Description |
|-----|------|-------------|
| `[OBJECT]` | String (optional) | Filter: component / operation id / log source / `all` |
| `--operation-id <ID>` | String | Match exact operation ID |
| `--kind <KIND>` | String | Restrict to `operation` or `component` |
| `--source <SOURCE>` | String | Match exact source |
| `--component <COMP>` | String | Match exact component name |
| `--severity <LEVEL>` | String | Minimum severity: `debug`, `info`, `warn`, `error` |
| `--since <ISO>` | String | Lexicographic ISO8601 lower bound on `started_at` |
| `--limit <N>` | Integer | Cap returned records (default 50, max 1000) |

### `restart`

Restart component services.

```
anolisa restart <COMPONENT>
```

| Arg | Type | Description |
|-----|------|-------------|
| `<COMPONENT>` | String | Component whose services to restart |

### `repair`

Refresh ANOLISA state from rpmdb.

```
anolisa repair <COMPONENT>
```

| Arg | Type | Description |
|-----|------|-------------|
| `<COMPONENT>` | String | Component whose ANOLISA state should be refreshed from rpmdb |

### `forget`

Drop ANOLISA state record.

```
anolisa forget <COMPONENT>
```

| Arg | Type | Description |
|-----|------|-------------|
| `<COMPONENT>` | String | Component whose ANOLISA state record should be dropped |

### `adopt`

Record an existing system RPM as ANOLISA-managed.

```
anolisa adopt <COMPONENT> [--package <NAME>]
```

| Arg | Type | Description |
|-----|------|-------------|
| `<COMPONENT>` | String | Component to record as an existing system RPM |
| `--package <NAME>` | String | Pin the RPM package name when ambiguous |

### `adapter`

Manage component adapters.

```
anolisa adapter scan
anolisa adapter enable <COMPONENT> [FRAMEWORK]
anolisa adapter disable <COMPONENT> [FRAMEWORK]
anolisa adapter status [COMPONENT]
```

Subcommands:

| Subcommand | Args | Description |
|------------|------|-------------|
| `scan` | *(none)* | Discover installed adapter declarations and local state |
| `enable` | `<COMPONENT>` (required), `[FRAMEWORK]` (optional) | Enable a component's adapter for a framework |
| `disable` | `<COMPONENT>` (required), `[FRAMEWORK]` (optional) | Disable a previously enabled adapter |
| `status` | `[COMPONENT]` (optional) | Report adapter receipt status |

---

## Management Commands

### `register`

Co-Build Program registration.

```
anolisa register [--yes]
anolisa register status [--json]
```

| Arg | Type | Description |
|-----|------|-------------|
| `--yes` | Flag | Skip interactive confirmation |

Subcommands:

| Subcommand | Args | Description |
|------------|------|-------------|
| `status` | `--json` (flag) | Output machine-readable JSON |

### `unregister`

Co-Build Program unregistration.

```
anolisa unregister [--force]
```

| Arg | Type | Description |
|-----|------|-------------|
| `--force` | Flag | Skip interactive confirmation |

### `env`

Display environment information and system capabilities.

```
anolisa env [--verbose]
```

| Arg | Type | Description |
|-----|------|-------------|
| `--verbose` | Flag | Include all probe details |

### `bug`

Generate a diagnostic report.

```
anolisa bug [--component <NAME>] [--limit <N>]
```

| Arg | Type | Description |
|-----|------|-------------|
| `--component <NAME>` | String | Limit report to one component |
| `--limit <N>` | Integer | Max recent warn/error log records (default 20, max 100) |

### `osbase`

OS base layer management.

```
anolisa osbase kernel install [--dry-run]
anolisa osbase kernel remove
anolisa osbase kernel status

anolisa osbase sandbox install <TARGET> [--dry-run] [--force] [--no-verify]
anolisa osbase sandbox uninstall <SCENARIO> [--dry-run]
anolisa osbase sandbox remove <TARGET> [--purge] [--dry-run]
anolisa osbase sandbox list [--json]
anolisa osbase sandbox status [TARGET] [--json]

anolisa osbase security install <TARGET> [--dry-run]
anolisa osbase security remove <TARGET>
anolisa osbase security status [TARGET]
```

#### kernel subcommands

| Subcommand | Args | Description |
|------------|------|-------------|
| `install` | `--dry-run` (flag) | Install kernel modules and eBPF programs |
| `remove` | *(none)* | Remove kernel modules |
| `status` | *(none)* | Show kernel substrate status |

#### sandbox subcommands

| Subcommand | Args | Description |
|------------|------|-------------|
| `install` | `<TARGET>` (required), `--dry-run`, `--force`, `--no-verify` | Install a sandbox scenario |
| `uninstall` | `<SCENARIO>` (required), `--dry-run` | Uninstall scenario packages |
| `remove` | `<TARGET>` (required), `--purge`, `--dry-run` | Remove a sandbox scenario |
| `list` | `--json` | List available sandbox scenarios |
| `status` | `[TARGET]` (optional), `--json` | Show sandbox scenario status |

#### security subcommands

| Subcommand | Args | Description |
|------------|------|-------------|
| `install` | `<TARGET>` (required), `--dry-run` | Install a security overlay |
| `remove` | `<TARGET>` (required) | Remove a security overlay |
| `status` | `[TARGET]` (optional) | Show security overlay status |

### `system`

System helper daemon management.

```
anolisa system serve [--socket <PATH>]
anolisa system setup [--helper-path <PATH>] [--upgrade]
anolisa system teardown
anolisa system status [--json]
```

| Subcommand | Args | Description |
|------------|------|-------------|
| `serve` | `--socket <PATH>` (default: system helper socket path) | Start the system helper daemon (foreground) |
| `setup` | `--helper-path <PATH>`, `--upgrade` | One-time setup: install system helper daemon |
| `teardown` | *(none)* | Remove system helper: stop service, delete unit + binary |
| `status` | `--json` | Check system helper health |

---

## Configuration

CLI configuration file: `~/.config/anolisa/config.toml`

```toml
[registry]
# Component registry URL
url = "https://registry.agentic-os.sh"

[install]
# Default install mode: "user" or "system"
mode = "user"

# Installation prefix for user mode
prefix = "~/.local"
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `ANOLISA_REGISTRY_URL` | Override registry URL |
| `ANOLISA_INSTALL_MODE` | Default install mode (`user` or `system`) |
| `ANOLISA_PREFIX` | Default install prefix |
| `ANOLISA_STATE_DIR` | Override state directory |
| `ANOLISA_NO_COLOR` | Disable colored output |
