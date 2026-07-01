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
- One of: cosh, OpenClaw, Hermes, claude-code, codex, qwencode (as the host Agent framework)

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

Tokenless integrates with Agent frameworks through adapter scripts or extensions.

### OpenClaw

Install the OpenClaw adapter:

```bash
/usr/share/anolisa/adapters/tokenless/openclaw/scripts/install.sh
```

The adapter registers as a middleware layer in the OpenClaw tool pipeline.

### Hermes

Install the Hermes adapter:

```bash
/usr/share/anolisa/adapters/tokenless/hermes/scripts/install.sh
```

### cosh (Copilot Shell)

For cosh, Tokenless is installed as an extension:

```bash
# Via Makefile target
make install-cosh-extension
```

This installs the extension to `~/.copilot-shell/extensions/tokenless/`.

### Other Adapters

Tokenless also supports claude-code, codex, and qwencode adapters. See `anolisa install tokenless` for available adapter options.

---

## CLI Commands

| Command | Description |
|---------|-------------|
| `tokenless compress-schema` | Compress tool schema definitions |
| `tokenless compress-response` | Compress CLI/tool response output |
| `tokenless compress-toon` | Compress to TOON format |
| `tokenless decompress-toon` | Decompress from TOON format |
| `tokenless env-check` | Check environment and integration status |
| `tokenless stats` | View compression statistics |

### View Compression Statistics

```bash
tokenless stats
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

No additional configuration is needed — metrics are exported automatically when `sls_enabled` is true.

---

## Configuration

Configuration file: `~/.tokenless/config.json`

This file is optional. When absent, all features are enabled by default.

```json
{
  "stats_enabled": true,
  "sls_enabled": true,
  "compression_enabled": true
}
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `stats_enabled` | boolean | `true` | Enable local statistics collection (stored in `~/.tokenless/stats.db`) |
| `sls_enabled` | boolean | `true` | Enable metrics export to AgentSight/SLS |
| `compression_enabled` | boolean | `true` | Enable compression (all-or-nothing toggle) |

### Environment Variable Overrides

Each config field can be overridden via environment variables:

- `TOKENLESS_STATS_ENABLED` — override `stats_enabled`
- `TOKENLESS_SLS_ENABLED` — override `sls_enabled`
- `TOKENLESS_COMPRESSION_ENABLED` — override `compression_enabled`

### Statistics Database

Local statistics are stored in `~/.tokenless/stats.db`.

---

## FAQ

**Q: Does Tokenless modify the actual tool behavior?**
A: No. Tokenless only compresses the representation sent to the model. Tool execution is unchanged.

**Q: Which frameworks are supported?**
A: cosh, OpenClaw, Hermes, claude-code, codex, and qwencode.

**Q: Can I disable compression?**
A: Yes. Set `compression_enabled` to `false` in `~/.tokenless/config.json` or set `TOKENLESS_COMPRESSION_ENABLED=false` in the environment. Compression is an all-or-nothing toggle — there is no per-tool exclusion.
