# Tokenless

Tokenless is ANOLISA's Token optimization component. It automatically compresses tool definitions and model response content without modifying business logic, significantly reducing Token consumption per conversation turn.

---

## Overview

AI Agent interactions typically include large volumes of tool schema definitions and verbose CLI output. Tokenless intercepts these at the framework level and applies lossless/near-lossless compression, delivering 30–70% Token savings transparently.

**Core Capabilities:**

- **Context Compression** — tool schema compaction, CLI response filtering, compact encoding
- **Statistics Tracking** — per-session and cumulative Token savings metrics
- **Transparent Integration** — plugs into existing Agent frameworks via hooks/plugins with zero code changes

---

## Prerequisites

- Linux (x86_64 or aarch64)
- One of: cosh, OpenClaw (as the host Agent framework)

---

## Installation

### Option 1: anolisa CLI (recommended)

```bash
anolisa install tokenless
```

### Option 2: YUM (Alinux, requires ANOLISA YUM repo)

```bash
sudo yum install tokenless
```

### Option 3: Source build (developers)

```bash
cd src/tokenless && cargo build --release
```

---

## Integration

Tokenless integrates with Agent frameworks through hooks or plugins.

### cosh (Copilot Shell)

Install the cosh hook:

```bash
/usr/share/tokenless/scripts/install.sh --cosh
```

Once installed, Tokenless automatically compresses tool schemas and CLI output within cosh sessions.

### OpenClaw

Install the OpenClaw plugin:

```bash
/usr/share/tokenless/scripts/install.sh --openclaw
```

The plugin registers as a middleware layer in the OpenClaw tool pipeline.

---

## Usage

### View Compression Statistics

```bash
tokenless stats list
```

Sample output:

```
Session       Tokens Saved   Ratio    Timestamp
────────────  ────────────   ─────    ──────────────────
sess-a3f1     12,480         62.3%    2025-06-30 14:22
sess-b7c2      8,912         48.7%    2025-06-30 15:01
────────────────────────────────────────────────────────
Total         21,392         56.1%
```

---

## AgentSight Integration

Tokenless reports compression metrics to AgentSight when both components are installed. View Token savings on the AgentSight web dashboard under the **Token Accounting** panel.

No additional configuration is needed — metrics are exported automatically.

---

## Configuration

Configuration file: `~/.config/tokenless/config.toml`

```toml
[compression]
# Compression level: "aggressive", "balanced", "conservative"
level = "balanced"

[stats]
# Enable statistics collection
enabled = true

[integration]
# Auto-export to AgentSight
agentsight = true
```

---

## FAQ

**Q: Does Tokenless modify the actual tool behavior?**
A: No. Tokenless only compresses the representation sent to the model. Tool execution is unchanged.

**Q: Which frameworks are supported?**
A: Currently cosh and OpenClaw. Hermes support is planned.

**Q: Can I disable compression for specific tools?**
A: Yes. Add tool names to the `[compression.exclude]` list in the config file.
