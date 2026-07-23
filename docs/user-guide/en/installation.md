# Installation Guide

This guide covers the progressive installation of ANOLISA — from the CLI tool to individual components and adapter setup.

---

## Step 1: Install the ANOLISA CLI

The `anolisa` CLI is the unified entry point for managing all ANOLISA components.

### Option A: Install script (recommended)

```bash
curl -fsSL https://agentic-os.sh | sh
```

### Option B: YUM (Alinux)

```bash
sudo yum install anolisa
```

After installation, verify:

```bash
anolisa --version
```

---

## Step 2: Environment Detection

Run the environment check to identify your system capabilities:

```bash
anolisa env
```

This displays:
- OS and architecture
- Available filesystems (btrfs for ws-ckpt)
- FUSE availability (for skillfs)
- Installed Agent runtimes (cosh, OpenClaw, Hermes)
- Kernel features (eBPF for agentsight)

---

## Step 3: Install Components

Install components individually based on your needs:

```bash
anolisa install <component>
```

### Available Components

| Component | Description | Supported modes |
|-----------|-------------|-----------------|
| `cosh` | Copilot Shell — AI terminal assistant | user, system |
| `os-skills` | System management and DevOps skills | user, system |
| `tokenless` | Token optimization (compression) | user, system |
| `ws-ckpt` | Workspace checkpoint/rollback | **system** |
| `skillfs` | FUSE virtual skill filesystem | **system** |
| `agent-memory` | MCP-based persistent memory | user, system |
| `agentsight` | eBPF tracing and dashboard | **system** |
| `agent-sec-core` | Security hardening | **system** |

> **Note**: System-only components require `sudo` and an explicit system scope:
> ```bash
> sudo anolisa --install-mode system install agentsight
> ```

### Install All Components

```bash
anolisa install --all
```

### YUM Alternative (Alinux)

For each component, you can also use YUM:

```bash
sudo yum install <component>
```

---

## Step 4: Adapter Setup

Adapters bridge components to specific Agent frameworks. Enable an adapter after installing the component:

```bash
anolisa adapter scan
anolisa adapter enable <component> [framework]
```

### Examples

```bash
# Tokenless hook for cosh
/usr/share/tokenless/scripts/install.sh --cosh

# Tokenless plugin for OpenClaw
/usr/share/tokenless/scripts/install.sh --openclaw

# ws-ckpt plugin for OpenClaw
ws-ckpt plugin install --runtime openclaw

# ws-ckpt plugin for Hermes
ws-ckpt plugin install --runtime hermes
```

---

## Step 5: Verify Installation

Check the status of all installed components:

```bash
anolisa status
```

Run the built-in diagnostic:

```bash
anolisa doctor
```

---

## Uninstallation

Remove a specific component:

```bash
anolisa uninstall <component>
```

There is no batch uninstall command. List the installed records, then remove
each intended component explicitly so its authority and package-removal policy
are reviewed independently:

```bash
anolisa list --installed
anolisa uninstall <component>
```

---

## Upgrade

Update a specific component:

```bash
anolisa update <component>
```

Update all installed components:

```bash
anolisa update all
```

`update all` updates recorded components but not the CLI binary. Use
`anolisa update self` for the CLI.

---

## Next Steps

- [anolisa CLI Reference](user-entrypoint/anolisa-cli.md)
- [Copilot Shell](user-entrypoint/copilot-shell/QUICKSTART.md)
- [Troubleshooting](troubleshooting.md)
