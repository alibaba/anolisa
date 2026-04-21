# agent-sec OpenClaw Plugin

OpenClaw security plugin that hooks into the agent lifecycle via `agent-sec-cli`, providing tool gating, code scanning, skill integrity verification, inbound filtering, prompt analysis, prompt guarding, and LLM output auditing.

---

## Prerequisites

| Dependency     | Version   | Check                        |
|----------------|-----------|------------------------------|
| Node.js        | >= 20     | `node --version`             |
| npm            | >= 10     | `npm --version`              |
| OpenClaw       | >= 0.8.0  | `openclaw --version`         |
| agent-sec-cli  | (latest)  | `agent-sec-cli --help`       |
| jq             | >= 1.6    | `jq --version`               |

---

## Project Structure

```
openclaw-plugin/
├── src/                        # TypeScript source
│   ├── index.ts                # Plugin entry point (definePluginEntry)
│   ├── types.ts                # SecurityCapability interface
│   ├── utils.ts                # CLI invocation utility (callAgentSecCli)
│   └── capabilities/           # One file per security capability
│       ├── tool-gate.ts        #   before_tool_call hook
│       ├── code-scan.ts        #   before_tool_call hook (exec commands)
│       ├── skill-ledger.ts     #   before_tool_call hook (SKILL.md reads)
│       ├── inbound-filter.ts   #   before_dispatch hook
│       ├── prompt-analyzer.ts  #   before_agent_reply hook
│       ├── prompt-guard.ts     #   before_prompt_build hook
│       └── llm-audit.ts        #   llm_output hook
├── tests/                      # Test utilities (not compiled into dist/)
│   ├── test-harness.ts         # Mock OpenClaw API for local testing
│   └── smoke-test.ts           # Smoke test for all capabilities
├── scripts/
│   └── deploy.sh               # Deployment and registration script
├── dist/                       # Compiled JS output (gitignored)
├── openclaw.plugin.json        # Plugin manifest
├── package.json
└── tsconfig.json
```

---

## Build

### Install Dependencies

```bash
cd src/agent-sec-core/openclaw-plugin
npm install
```

### Compile TypeScript

```bash
npm run build
```

This runs `tsc --project tsconfig.json` and outputs compiled JS to `dist/`.

### Verify Build Output

```bash
ls dist/
# Expected: capabilities/  index.js  index.d.ts  types.js  types.d.ts  utils.js  utils.d.ts
```

> **Note:** Test files in `tests/` are excluded from `dist/` since they live outside `src/`.

---

## Deploy to OpenClaw

### Option A: Deploy from Source (Development)

Point `deploy.sh` directly at the source directory:

```bash
# Build first
npm run build

# Deploy — pass the plugin directory as argument
./scripts/deploy.sh "$(pwd)"
```

### Option B: Deploy from Packaged Tarball

```bash
# Create tarball
npm run pack
# Output: agent-sec-openclaw-plugin-0.3.0.tgz

# Extract to target directory
mkdir -p /opt/agent-sec/openclaw-plugin
tar -xzf agent-sec-openclaw-plugin-0.3.0.tgz \
    --strip-components=1 \
    -C /opt/agent-sec/openclaw-plugin

# Deploy
./scripts/deploy.sh /opt/agent-sec/openclaw-plugin
```

### Option C: Install via Makefile (Development/Testing)

```bash
# From agent-sec-core root directory
cd src/agent-sec-core

# Build the plugin
make build-openclaw-plugin

# Install files to /opt/agent-sec/openclaw-plugin/
sudo make install-openclaw-plugin

# Register the plugin with OpenClaw
sudo /opt/agent-sec/openclaw-plugin/scripts/deploy.sh /opt/agent-sec/openclaw-plugin

# Restart gateway to load the plugin
sudo systemctl restart openclaw-gateway
```

> **Note:** `make install-openclaw-plugin` only copies files. You must run `deploy.sh` separately to register the plugin.

---

## What `deploy.sh` Does

The deployment script performs these steps:

1. **Pre-checks** — Verifies `openclaw` and `agent-sec-cli` are in PATH; validates `openclaw.plugin.json` and `dist/` exist
2. **Plugin installation** — Runs `openclaw plugins install <path> --force --dangerously-force-unsafe-install` to register the plugin
3. **User guidance** — Displays instructions to restart the OpenClaw gateway (does NOT restart automatically)

> **Important:** `deploy.sh` only registers the plugin with OpenClaw config. It does **NOT** start/stop/restart the gateway service.
> 
> To restart the gateway:
> ```bash
> sudo systemctl restart openclaw-gateway  # If using systemd
> # Or manually restart your gateway process
> ```

### Custom Config Path

```bash
OPENCLAW_CONFIG=~/.openclaw-dev/openclaw.json ./scripts/deploy.sh "$(pwd)"
```

---

## Verify Installation

After deployment, verify the plugin is loaded:

```bash
openclaw plugins inspect agent-sec
```

Expected output:

```
Agent Security
id: agent-sec
Security hooks powered by agent-sec-cli

Status: loaded
Version: 0.3.0
Source: ~/path/to/openclaw-plugin/dist/index.js

Typed hooks:
before_agent_reply (priority 150)
before_dispatch (priority 200)
before_prompt_build (priority 50)
before_tool_call (priority 100)
llm_output (priority 0)
```

---

## Testing

### Smoke Test (Mock Mode)

Runs all 7 capabilities against mock events without requiring a real `agent-sec-cli` installation:

```bash
npm run smoke
```

### Smoke Test (Live Mode)

Runs against the real `agent-sec-cli` binary:

```bash
AGENT_SEC_LIVE=1 npm run smoke
```

---

## Plugin Capabilities

| Capability         | Hook                  | Priority | Behavior                                             |
|--------------------|-----------------------|----------|------------------------------------------------------|
| `tool-gate`        | `before_tool_call`    | 100      | Gates tool execution; blocks if risk threshold met   |
| `code-scan`        | `before_tool_call`    | 80       | Scans shell commands for security issues             |
| `skill-ledger`     | `before_tool_call`    | 80       | Checks skill integrity when SKILL.md is read         |
| `inbound-filter`   | `before_dispatch`     | 200      | Scans inbound messages; blocks high-risk content     |
| `prompt-analyzer`  | `before_agent_reply`  | 150      | Detects prompt injection attacks                     |
| `prompt-guard`     | `before_prompt_build` | 50       | Injects security policy into prompt context          |
| `llm-audit`        | `llm_output`          | 0        | Fire-and-forget audit logging of LLM responses       |

### Disabling Individual Capabilities

Configure via OpenClaw plugin settings:

```json
{
  "capabilities": {
    "tool-gate": { "enabled": false },
    "llm-audit": { "enabled": false }
  }
}
```

### Configuring `skill-ledger`

The `skill-ledger` capability checks skill integrity by invoking `agent-sec-cli skill-ledger check` when the agent reads a `SKILL.md` file. It automatically initializes signing keys on first use.

**Prerequisites**: `agent-sec-cli skill-ledger check` must be available. Signing keys are auto-initialized (no passphrase) if not present.

---

## Upgrade

To upgrade the plugin to a new version:

### Development Environment

```bash
cd src/agent-sec-core/openclaw-plugin

# Pull latest changes
git pull

# Rebuild
npm install
npm run build

# Re-register plugin (updates to new version)
./scripts/deploy.sh "$(pwd)"

# Restart gateway
sudo systemctl restart openclaw-gateway
```

### Production Environment (Installed via Makefile)

```bash
cd src/agent-sec-core

# Rebuild and install files
make build-openclaw-plugin
sudo make install-openclaw-plugin

# Re-register plugin
sudo /opt/agent-sec/openclaw-plugin/scripts/deploy.sh /opt/agent-sec/openclaw-plugin

# Restart gateway
sudo systemctl restart openclaw-gateway
```

The `openclaw plugins install --force` command automatically updates the plugin to the new version. Other plugins are unaffected.
