---
name: install-openclaw
description: Install and configure OpenClaw non-interactively with Alibaba Cloud Model Studio. Use when the user asks to install OpenClaw, configure Aliyun Bailian/Model Studio/DashScope credentials, choose pay-as-you-go, Coding Plan, or Token Plan billing, set Base URL/model config, optionally configure DingTalk, start the local gateway service, or troubleshoot OpenClaw model/auth/channel setup.
---

# OpenClaw Non-Interactive Setup

Use this skill to turn a user's one-sentence request into a complete local OpenClaw setup. The normal path is one script command: install Node.js/npm/OpenClaw when needed, write Alibaba Cloud Model Studio config, install/restart the local gateway service, and verify `openclaw gateway health`.

Do not use `openclaw onboard` unless the user explicitly asks for interactive setup. Do not configure DingTalk unless the user provides DingTalk credentials or asks for DingTalk access.

## Billing

Map user wording to `--billing`:

| User says | Use |
|---|---|
| 按量付费, 后付费, DashScope, 百炼 API Key | `payg` |
| Coding Plan, 订阅 coding, Coding Plan 专属 API Key | `coding` |
| Token Plan, 团队版, token 订阅 | `token` |

Do not infer Coding Plan or Token Plan only from a raw API key string. A normal
Alibaba Cloud Model Studio/DashScope/百炼 API Key is pay-as-you-go unless the
user explicitly says Coding Plan or Token Plan.

Default to `--billing payg --region china` when the user does not specify billing or region. If the user gives no API key and no usable environment variable, ask for the key and include the matching source URL:

- Pay-as-you-go: https://help.aliyun.com/zh/model-studio/get-api-key
- Coding Plan: https://bailian.console.aliyun.com/cn-beijing/?tab=model#/efm/coding_plan
- Token Plan: https://bailian.console.aliyun.com/?tab=plan#/efm/subscription/overview

Use `--region singapore` only for pay-as-you-go Singapore. For deeper billing details, read `references/aliyun-model-studio-openclaw.md`.

## Execute

If the user asks to install OpenClaw but has not provided an API key, run the
basic dependency precheck first:

```bash
python3 /home/ecs-user/.copilot-shell/skills/install-openclaw/scripts/install_openclaw.py \
  --billing payg \
  --precheck-only
```

After precheck passes, tell the user the matching API key source URL printed by
the script. If the user already provided an API key, run the full install
directly; the script performs the same dependency precheck before installing.

Run exactly this script from the installed skill directory for full install:

```bash
python3 /home/ecs-user/.copilot-shell/skills/install-openclaw/scripts/install_openclaw.py \
  --billing payg \
  --api-key "$BAILIAN_API_KEY"
```

If this skill is installed under a different agent home, keep the same relative script path:

```bash
python3 <install-openclaw-skill-dir>/scripts/install_openclaw.py ...
```

The authoritative implementation is `scripts/install_openclaw.py`.
The normal parameters are `--billing`, `--api-key`, `--api-key-env`, `--region`,
`--model-id`, `--npm-registry`, `--precheck-only`, `--skip-tokenless`, and DingTalk-specific flags.

## Examples

Pay-as-you-go:

```bash
python3 /home/ecs-user/.copilot-shell/skills/install-openclaw/scripts/install_openclaw.py \
  --billing payg \
  --api-key "$BAILIAN_API_KEY"
```

Coding Plan:

```bash
python3 /home/ecs-user/.copilot-shell/skills/install-openclaw/scripts/install_openclaw.py \
  --billing coding \
  --api-key "$CODING_PLAN_API_KEY"
```

Token Plan:

```bash
python3 /home/ecs-user/.copilot-shell/skills/install-openclaw/scripts/install_openclaw.py \
  --billing token \
  --api-key "$BAILIAN_TOKEN_PLAN_API_KEY"
```

Prefer `--api-key-env NAME` when an environment variable already contains the key:

```bash
python3 /home/ecs-user/.copilot-shell/skills/install-openclaw/scripts/install_openclaw.py \
  --billing payg \
  --api-key-env BAILIAN_API_KEY
```

With DingTalk:

```bash
python3 /home/ecs-user/.copilot-shell/skills/install-openclaw/scripts/install_openclaw.py \
  --billing coding \
  --api-key-env CODING_PLAN_API_KEY \
  --dingtalk-client-id "dingxxxxxx" \
  --dingtalk-client-secret "$DINGTALK_CLIENT_SECRET" \
  --install-dingtalk-plugin
```

## What The Script Does

- Checks Node.js/npm and installs them with `dnf`/`yum` when needed.
- Runs dependency precheck before installing or writing config.
- Installs OpenClaw with npm unless `--skip-install-openclaw` is passed.
- Uses npm registry `https://registry.npmmirror.com` by default; override with `--npm-registry`.
- Writes only OpenClaw schema-supported config fields.
- Writes Alibaba Cloud Model Studio config with `api = anthropic-messages`.
- Sets `gateway.mode = local`, `gateway.bind = loopback`, and `gateway.auth.mode = none` for local single-machine setup.
- Starts OpenClaw through `openclaw gateway install` and `openclaw gateway restart` unless `--skip-gateway` is passed.
- If the gateway port is occupied by OpenClaw, it clears that stale listener. If the port is occupied by another process, it stops and prints the process details for the user to decide.
- **Auto-installs the tokenless OpenClaw plugin** after OpenClaw is installed, for token usage tracking and response compression. Pass `--skip-tokenless` to opt out.

## Verify

The script prints useful checks. Prefer:

```bash
openclaw models list
openclaw gateway health
openclaw status
```

Only run `openclaw agent --message "hello" --agent main` as an optional model smoke test after gateway health passes. Do not treat `EMBEDDED FALLBACK` as success; if it mentions `pairing required` or `scope upgrade pending approval`, read `references/troubleshooting.md`.

## References

- Standard Alibaba Cloud OpenClaw guide: https://help.aliyun.com/zh/model-studio/openclaw
- Read `references/aliyun-model-studio-openclaw.md` for the billing matrix and API key source URLs.
- Read `references/dingtalk-setup-guide.md` only when configuring DingTalk.
- Read `references/troubleshooting.md` when model/auth/channel checks fail.
