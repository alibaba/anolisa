# Installation Guide

[中文版](../zh/installation.md)

This guide covers the progressive installation of ANOLISA — from the CLI tool to individual components and adapter setup.

---

## Step 1: Install the ANOLISA CLI

The `anolisa` CLI is the unified entry point for managing all ANOLISA components.

### Option A: Install script (recommended)

```bash
curl -fsSL https://get.agentic-os.sh | bash
```

### Option B: YUM (Alinux)

```bash
sudo yum install anolisa
```

After installation, verify:

```bash
anolisa --version
```

Before installing a component that uses the current raw package contract,
update an older CLI through the tool that owns it:

```bash
# CLI installed by get.agentic-os.sh
anolisa update self

# RPM-owned CLI
sudo anolisa update self
```

AgentSecCore requires `anolisa` 0.2.17 or later. The CLI reports an update hint
and stops before changing the host when it cannot safely read a package
contract.

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
| `cosh-ng` | AI-native Linux terminal and deterministic Agent runtime (experimental) | **system** |
| `os-skills` | System management and DevOps skills | user, system |
| `tokenless` | Token optimization (compression) | user, system |
| `ws-ckpt` | Workspace checkpoint/rollback | **system** |
| `skillfs` | FUSE virtual skill filesystem | **system** |
| `agent-memory` | MCP-based persistent memory | user, system |
| `agentsight` | eBPF tracing and dashboard | **system** |
| `sec-core` | Local security runtime, scanners, and adapters | **system** |

> **Note**: System-only components require `sudo` and an explicit system scope:
> ```bash
> sudo anolisa --install-mode system install agentsight
> ```

Install cosh-ng in system mode, then start the terminal with `cosh`:

```bash
sudo anolisa --install-mode system install cosh-ng
cosh
```

Use the component name `sec-core` with the ANOLISA CLI. The Alinux RPM keeps
its package name `agent-sec-core`:

```bash
sudo anolisa --install-mode system install sec-core

# If you install the RPM directly, let ANOLISA track it before adapter setup
sudo yum install anolisa agent-sec-core
sudo anolisa --install-mode system adopt sec-core
```

Continue to [Step 4](#step-4-adapter-setup), then run
`anolisa adapter enable sec-core <framework>` as the user who owns the target
Agent configuration.

### Install All Components

```bash
anolisa install --all
```

### YUM Alternative (Alinux)

For each component, you can also use YUM. Install the system CLI in the same
transaction so sudo can find it without relying on a user-local PATH. A direct
RPM installation does not create an ANOLISA state record, so adopt it before
using lifecycle or adapter commands:

```bash
sudo yum install anolisa <rpm-package>
sudo anolisa --install-mode system adopt <component>
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

# AgentSecCore plugin for OpenClaw
anolisa adapter enable sec-core openclaw
```

The system installation owns the component files. Adapter enablement runs as
the user who owns the target Agent configuration, so it does not need `sudo`
for a normal user-scoped framework installation.

---

## Step 5: Start Long-running Services

Installing a component and starting its resident service are separate actions.
AgentSight installs `agentsight.service` and its enforcer dependency without
enabling either unit. Start the main unit when the machine is ready to collect
events:

```bash
sudo systemctl enable --now agentsight.service
sudo systemctl status agentsight.service
```

The main unit runs eBPF tracing and the Dashboard together as root and keeps its
data private under `/var/log/sysak/.agentsight`. Use `sudo` for commands that
query service data. For foreground troubleshooting, stop the unit first, then
run the tracer and server as root in separate terminals:

```bash
sudo systemctl stop agentsight.service

# Terminal 1
sudo agentsight trace

# Terminal 2
sudo agentsight serve
```

---

## Step 6: Verify Installation

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
`anolisa update self` for a script-installed CLI or `sudo anolisa update self`
for an RPM-owned CLI.

---

## Next Steps

- [anolisa CLI Reference](user-entrypoint/anolisa-cli.md)
- [cosh-ng Quick Start](user-entrypoint/cosh-ng/QUICKSTART.md)
- [Copilot Shell](user-entrypoint/copilot-shell/QUICKSTART.md)
- [Troubleshooting](troubleshooting.md)
