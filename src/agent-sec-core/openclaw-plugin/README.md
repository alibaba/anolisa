# agent-sec OpenClaw Plugin

OpenClaw security plugin that hooks into the agent lifecycle via `agent-sec-cli`, providing code scanning, skill integrity verification, prompt analysis, PII checking, and best-effort agent observability logging.

---

## Prerequisites

| Dependency     | Version   | Check                        |
|----------------|-----------|------------------------------|
| Node.js        | >= 20     | `node --version`             |
| npm            | >= 10     | `npm --version`              |
| OpenClaw       | Typed plugin runtime | `openclaw --version`       |
| agent-sec-cli  | (latest)  | `agent-sec-cli --help`       |
| jq             | >= 1.6    | `jq --version`               |

Development and test builds use the `openclaw` dev dependency pinned in `package.json` so TypeScript can compile against the newest typed hook definitions. The OpenClaw runtime does not need to match that dev dependency. Runtime compatibility is capability-based: older runtimes that do not know a typed hook ignore that hook registration with a diagnostic instead of crashing the gateway.

---

## Project Structure

```
openclaw-plugin/
├── src/                        # TypeScript source
│   ├── index.ts                # Plugin entry point (definePluginEntry)
│   ├── types.ts                # SecurityCapability interface
│   ├── utils.ts                # CLI invocation utility (callAgentSecCli)
│   ├── capabilities/           # Security capability entry files
│   │   ├── skill-ledger.ts     #   before_tool_call
│   │   ├── code-scan.ts        #   before_tool_call hook
│   │   ├── prompt-scan.ts      #   before_dispatch hook
│   │   ├── pii-scan.ts         #   before_prompt_build + reply_dispatch hooks
│   │   └── observability.ts    #   observability hook registration
│   └── helpers/                # Capability support code
│       └── observability/      #   OpenClaw → agent-sec observability adapter
│           ├── schema.ts       #     hook mapping + metric allowlist
│           ├── record.ts       #     record assembly + metadata validation
│           ├── metrics.ts      #     hook-specific metric extraction
│           ├── extractors.ts   #     response/error extraction helpers
│           ├── helpers.ts      #     generic parsing helpers
│           └── types.ts        #     shared observability types
├── tests/                      # Test utilities (not compiled into dist/)
│   ├── test-harness.ts         # Mock OpenClaw API for local testing
│   ├── smoke-test.ts           # Smoke test for all capabilities
│   └── unit/                   # Unit tests
│       ├── code-scan-test.ts   #   scan-code handler tests
│       ├── observability-test.ts # observability handler tests
│       └── skill-ledger-test.ts #  skill-ledger handler tests
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
# Output: agent-sec-openclaw-plugin-0.x.y.tgz

# Extract to target directory
mkdir -p /opt/agent-sec/openclaw-plugin
tar -xzf agent-sec-openclaw-plugin-0.x.y.tgz \
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
openclaw gateway restart
```

> **Note:** `make install-openclaw-plugin` only copies files. You must run `deploy.sh` separately to register the plugin.

---

## What `deploy.sh` Does

The deployment script performs these steps:

1. **Pre-checks** — Verifies `openclaw` and `agent-sec-cli` are in PATH; validates `openclaw.plugin.json` and `dist/` exist
2. **Plugin installation** — Runs `openclaw plugins install <path> --force --dangerously-force-unsafe-install` to register the plugin
3. **Conversation access policy** — Sets `plugins.entries.agent-sec.hooks.allowConversationAccess=true` so conversation observability hooks can register
4. **User guidance** — Displays instructions to restart the OpenClaw gateway (does NOT restart automatically)

> **Important:** `deploy.sh` installs the plugin and applies required OpenClaw config. It does **NOT** start/stop/restart the gateway service.
> 
> To restart the gateway:
> ```bash
> openclaw gateway restart  # Recommended: OpenClaw CLI
> # Or
> systemctl --user restart openclaw-gateway-dev.service  # If using systemd user service
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
Version: 0.x.y
Source: ~/path/to/openclaw-plugin/dist/index.js

Typed hooks:
before_dispatch (priority 190)
before_prompt_build (priority 0)
reply_dispatch (priority 0)
llm_input (priority 1000)
model_call_started (priority 1000)
model_call_ended (priority 1000)
llm_output (priority 1000)
agent_end (priority 1000)
before_tool_call (priority 80)
before_tool_call (priority 0)
before_tool_call (priority -10000)
after_tool_call (priority 1000)
```

Also check the plugin is activated by gateway after openclaw **v2026.4.25**
```
openclaw gateway call health --params '{"probe":true}' --json | jq -e '(.plugins.loaded // []) | index("agent-sec") != null'
```
Expected output:
```
true
```
---

## Testing

### Smoke Test (Mock Mode)

Runs all capabilities against mock events without requiring a real `agent-sec-cli` installation:

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
| `prompt-scan`      | `before_dispatch`     | 190      | Scans inbound messages for prompt injection attacks   |
| `pii-scan-user-input` | `before_prompt_build`, `reply_dispatch` | 0 (default) | Scans current user prompt for PII/credentials and emits a non-blocking same-run warning |
| `scan-code`        | `before_tool_call`    | 0 (default) | Scans tool commands for security issues              |
| `skill-ledger`     | `before_tool_call`    | 80       | Checks skill integrity when SKILL.md is read         |
| `observability`    | selected typed hooks  | varies   | Sends observability records to agent-sec-cli          |

### Configuring `code-scan`

The `scan-code` capability intercepts `exec` tool calls and scans commands via `agent-sec-cli scan-code`. By default, security issues are logged (`api.logger.warn`) but the tool call is allowed to proceed. This avoids blocking TUI users who cannot see Dashboard approval cards.

Set `codeScanRequireApproval: true` to enable approval mode, which pops a confirmation card on the Dashboard for `warn` and `deny` verdicts:

```bash
openclaw config set plugins.entries.agent-sec.config.codeScanRequireApproval true
```

### Configuring `pii-scan-user-input`

The `pii-scan-user-input` capability scans only `event.prompt` in `before_prompt_build`. It intentionally does not scan `event.messages`, because that list may include history, tool results, memory, or RAG context and can repeatedly warn on older PII that was not submitted in the current turn.

`warn` and `deny` verdicts never block OpenClaw in v1. The capability caches a minimal warning under the current `runId`, then `reply_dispatch` reads that warning and queues it with `dispatcher.sendBlockReply({ text })` before the default agent reply flow continues. In this context, block reply means OpenClaw's intermediate reply type, not a security block. The warning is removed only after it is successfully queued. This avoids consuming the warning in a generic outbound `message_sending` hook that may not represent the final user-visible reply. If `runId` is missing, the capability fails open and does not cache a session-level warning. If OpenClaw marks the turn with `sendPolicy: "deny"` or `suppressUserDelivery: true`, the warning is dropped without display so the plugin does not override host-level delivery policy.

### Configuring `observability`

The `observability` capability is enabled by default and invokes:

```bash
agent-sec-cli observability record --format json --stdin
```

Each hook emits one JSON record with `hook`, `observedAt`, `metadata`, and hook-specific `metrics`. The plugin registers OpenClaw hook names, but sends the generic `agent-sec-cli` hook name in `payload.hook`. Failures, missing CLI, malformed output, and timeouts are fail-open and never block OpenClaw behavior.

OpenClaw runtimes that expose `model_call_started` and `model_call_ended` provide model-call telemetry. Older runtimes load the plugin but skip unknown telemetry sources. Newer OpenClaw versions may provide richer fields on those hooks; the plugin sends whichever accepted metrics are present.

Observed hooks and metrics:

| OpenClaw hook | agent-sec-cli hook | Metrics sent |
|---------------|--------------------|--------------|
| `llm_input` | `before_agent_run` | `prompt`, `system_prompt`, `user_input`, `history_messages_count`, `images_count`, `context_window_utilization`, `model_id`, `model_provider` |
| `model_call_started` | `before_llm_call` | `model_id`, `model_provider`, `api`, `transport` |
| `model_call_ended` | `after_llm_call` | `latency_ms`, `outcome`, `error_category`, `failure_kind`, `request_payload_bytes`, `response_stream_bytes`, `time_to_first_byte_ms`, `upstream_request_id_hash` |
| `llm_output` | `after_agent_run` | `response`, `output_kind`, `stop_reason`, `assistant_texts_count`, `tool_calls_count`, `tool_calls` |
| `before_tool_call` | `before_tool_call` | `tool_name`, `parameters` |
| `after_tool_call` | `after_tool_call` | `result`, `error`, `duration_ms`, `status`, `exit_code`, `result_size_bytes` |
| `agent_end` | `after_agent_run` | `success`, `error`, `duration_ms`, `total_api_calls`, `total_tool_calls`, `final_model_id`, `final_model_provider` |

If an OpenClaw hook does not provide required metadata or any metric accepted by the current `agent-sec-cli` schema, the plugin skips the record instead of sending an invalid payload.
`llm_input` and `llm_output` are run-level OpenClaw hooks in current runtimes, so the plugin maps them to `before_agent_run` and `after_agent_run`. Per-call telemetry remains on `model_call_started` and `model_call_ended`.
`agent_end` records run status and aggregate counters only; final response content comes from `llm_output`.

Supported OpenClaw plugin entry config:

```json
{
  "plugins": {
    "entries": {
      "agent-sec": {
        "config": {
          "promptScanBlock": false,
          "codeScanRequireApproval": false,
          "piiScanUserInput": true,
          "piiIncludeLowConfidence": false,
          "piiWarningTtlMs": 300000,
          "capabilities": {
            "scan-code": { "enabled": true },
            "prompt-scan": { "enabled": true },
            "pii-scan-user-input": { "enabled": true },
            "skill-ledger": { "enabled": true },
            "observability": { "enabled": true }
          }
        },
        "hooks": {
          "allowConversationAccess": true
        }
      }
    }
  }
}
```

Set a capability's `enabled` value to `false` to skip registering only that capability while keeping the rest of the `agent-sec` plugin active.

`llm_input`, `llm_output`, and `agent_end` require OpenClaw to allow conversation access for this external plugin with `plugins.entries.agent-sec.hooks.allowConversationAccess=true`. Without that OpenClaw setting, those hooks are blocked by OpenClaw before this plugin sees them.

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
openclaw gateway restart
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
openclaw gateway restart
```

The `openclaw plugins install --force` command automatically updates the plugin to the new version. Other plugins are unaffected.
