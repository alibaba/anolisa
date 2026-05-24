#!/usr/bin/env bash
# detect-openclaw.sh — Inspect ws-ckpt OpenClaw integration. Read-only.
#
# Reports OpenClaw CLI, ws-ckpt plugin install state, ws-ckpt runtime binary,
# and adapter plugin/skill source availability. Exits 0 when the OpenClaw CLI
# and the ws-ckpt plugin are both present; non-zero when either is missing.

set -euo pipefail

# shellcheck source=lib-discover.sh
source "$(dirname "$0")/lib-discover.sh"

COMPONENT="${ANOLISA_COMPONENT:-ws-ckpt}"
AGENT="${ANOLISA_TARGET:-openclaw}"
OPENCLAW_HOME="${OPENCLAW_HOME:-$HOME/.openclaw}"
OPENCLAW_BIN="${OPENCLAW_BIN:-}"
OPENCLAW_SKILLS_DIR="${OPENCLAW_SKILLS_DIR:-${OPENCLAW_HOME%/}/skills}"
export PATH="$HOME/.local/bin:${OPENCLAW_HOME%/}/bin:/usr/local/bin:$PATH"

PLUGIN_ID="ws-ckpt"

line()  { printf '[%s] %s\n' "$COMPONENT" "$*"; }
field() { printf '[%s]   %-26s %s\n' "$COMPONENT" "$1" "$2"; }

MISSING=()
note_missing() { MISSING+=("$1"); }

if [ -z "$OPENCLAW_BIN" ]; then
    OPENCLAW_BIN="$(command -v openclaw 2>/dev/null || true)"
fi

line "${AGENT} detect"
if [ -n "$OPENCLAW_BIN" ] && [ -x "$OPENCLAW_BIN" ]; then
    field "openclaw CLI" "present (${OPENCLAW_BIN})"
else
    field "openclaw CLI" "missing"
    note_missing "openclaw CLI"
fi

plugin_state="missing"
plugin_detail="$PLUGIN_ID"
if [ -n "$OPENCLAW_BIN" ] && [ -x "$OPENCLAW_BIN" ]; then
    plugins_json="$(env -u OPENCLAW_HOME "$OPENCLAW_BIN" plugins list --json 2>/dev/null || true)"
    plugins_txt="$(env -u OPENCLAW_HOME "$OPENCLAW_BIN" plugins list 2>/dev/null || true)"
    if grep -qE "\"id\"[[:space:]]*:[[:space:]]*\"${PLUGIN_ID}\"" <<<"$plugins_json" \
       || grep -qE "(^|[[:space:]])${PLUGIN_ID}([[:space:]]|$)" <<<"$plugins_txt"; then
        plugin_state="listed"
        plugin_detail="$PLUGIN_ID (openclaw plugins list)"
    fi
fi
if [ "$plugin_state" = "missing" ] && [ -d "${OPENCLAW_HOME%/}/extensions/${PLUGIN_ID}" ]; then
    plugin_state="installed"
    plugin_detail="${OPENCLAW_HOME%/}/extensions/${PLUGIN_ID}"
fi
if [ "$plugin_state" != "missing" ]; then
    field "${PLUGIN_ID} plugin" "${plugin_state} (${plugin_detail})"
else
    field "${PLUGIN_ID} plugin" "missing"
    note_missing "${PLUGIN_ID} plugin"
fi

# Skill fallback — only informational; install path prefers the plugin.
skill_dst="${OPENCLAW_SKILLS_DIR%/}/${PLUGIN_ID}"
if [ -f "$skill_dst/SKILL.md" ]; then
    field "skill fallback" "present (${skill_dst})"
else
    field "skill fallback" "missing (${skill_dst})"
fi

# Runtime binary — ws-ckpt CLI used by the plugin's snapshot operations.
runtime_bin="$(command -v ws-ckpt 2>/dev/null || true)"
if [ -n "$runtime_bin" ]; then
    field "ws-ckpt binary" "present (${runtime_bin})"
else
    field "ws-ckpt binary" "missing"
    note_missing "ws-ckpt binary"
fi

# Adapter source resources — plugin and skill source for re-install.
plugin_src="$(find_plugin_src openclaw 2>/dev/null || true)"
field "plugin resource" "${plugin_src:--}"
skill_src="$(find_skill_src 2>/dev/null || true)"
field "skill resource" "${skill_src:--}"

if [ ${#MISSING[@]} -gt 0 ]; then
    line "${AGENT}: not ready (missing: ${MISSING[*]})"
    exit 1
fi
line "${AGENT}: ready"
exit 0
