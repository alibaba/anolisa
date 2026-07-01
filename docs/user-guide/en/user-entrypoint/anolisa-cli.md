# anolisa CLI

The `anolisa` CLI is the unified command-line interface for managing all ANOLISA components. It handles installation, updates, status monitoring, and diagnostics.

---

## Installation

### Option A: Install script (recommended)

```bash
curl -fsSL https://agentic-os.sh | sh
```

### Option B: YUM (Alinux)

```bash
sudo yum install anolisa
```

Verify installation:

```bash
anolisa --version
```

---

## Commands

### install

Install a component:

```bash
anolisa install <component>

# Install with system mode (root required)
sudo anolisa install <component>

# Install all components
anolisa install --all
```

### uninstall

Remove a component:

```bash
anolisa uninstall <component>

# Remove all components
anolisa uninstall --all
```

### update

Update installed components:

```bash
anolisa update <component>

# Update all
anolisa update --all
```

### status

Show installation status of components:

```bash
anolisa status

# Status of a specific component
anolisa status <component>
```

### list

List all available components:

```bash
anolisa list
```

### env

Display environment information and system capabilities:

```bash
anolisa env
```

### doctor

Run health checks on installed components:

```bash
anolisa doctor

# Check specific category
anolisa doctor --check <category>
```

Categories: `network`, `build-deps`, `ebpf`, `fuse`, `btrfs`

### adapter

Manage component adapters:

```bash
# Discover adapters
anolisa adapter scan

# Enable an adapter
anolisa adapter enable <component> [framework]

# Disable an adapter
anolisa adapter disable <component> [framework]

# Check adapter status
anolisa adapter status [component]
```

### logs

View component logs:

```bash
anolisa logs <component>
anolisa logs <component> --follow
anolisa logs <component> --tail 50
```

### bug

Generate a diagnostic report:

```bash
anolisa bug
```

---

## Global Options

| Option | Description |
|--------|-------------|
| `--verbose` | Enable verbose output |
| `--quiet` | Suppress non-error output |
| `--version` | Show CLI version |
| `--help` | Show help information |

---

## Examples

```bash
# Full setup workflow
curl -fsSL https://agentic-os.sh | sh
anolisa env
anolisa install cosh
anolisa install tokenless
anolisa adapter enable tokenless cosh
anolisa doctor
anolisa status
```

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

---

## See Also

- [Installation Guide](../installation.md)
- [Troubleshooting](../troubleshooting.md)
