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

| Component | Description | Mode |
|-----------|-------------|------|
| `cosh` | Copilot Shell — AI terminal assistant | user |
| `os-skills` | System management and DevOps skills | user |
| `tokenless` | Token optimization (compression) | user |
| `ws-ckpt` | Workspace checkpoint/rollback | user |
| `skillfs` | FUSE virtual skill filesystem | user |
| `agent-memory` | MCP-based persistent memory | user |
| `agentsight` | eBPF tracing and dashboard | **system** |
| `agent-sec-core` | Security hardening | **system** |

> **Note**: Components marked **system** require `sudo`:
> ```bash
> sudo anolisa install agentsight
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

Adapters bridge components to specific Agent frameworks. Install adapters after installing the component:

```bash
anolisa adapter install <component> --runtime <runtime>
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

Remove all ANOLISA components:

```bash
anolisa uninstall --all
```

---

## Upgrade

Update a specific component:

```bash
anolisa update <component>
```

Update all installed components:

```bash
anolisa update --all
```

---

## Next Steps

- [anolisa CLI Reference](user-entrypoint/anolisa-cli.md)
- [Copilot Shell](user-entrypoint/copilot-shell/QUICKSTART.md)
- [Troubleshooting](troubleshooting.md)
