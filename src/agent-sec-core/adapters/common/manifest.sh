#!/usr/bin/env bash
# Shared manifest helpers for sec-core adapter scripts.

SEC_CORE_DEFAULT_SKILLS=(code-scanner prompt-scanner skill-ledger)

sec_core_default_plugin_id() {
    case "$1" in
        openclaw) printf '%s\n' "agent-sec" ;;
        hermes)   printf '%s\n' "agent-sec-core-hermes-plugin" ;;
        *)        printf '%s\n' "" ;;
    esac
}

sec_core_manifest_plugin_id() {
    local target="$1" manifest="$2" default_id="${3:-}"
    local value=""

    if [ -z "$default_id" ]; then
        default_id="$(sec_core_default_plugin_id "$target")"
    fi

    if [ -n "$manifest" ] && [ -f "$manifest" ] && command -v jq >/dev/null 2>&1; then
        value="$(jq -r --arg target "$target" \
            '.targets[$target].capabilities.plugins[0] // empty' \
            "$manifest" 2>/dev/null || true)"
    fi

    if [ -n "$value" ]; then
        printf '%s\n' "$value"
    else
        printf '%s\n' "$default_id"
    fi
}

sec_core_manifest_skills() {
    local target="$1" manifest="$2"
    shift 2
    local defaults=("$@")
    local values=""

    if [ "${#defaults[@]}" -eq 0 ]; then
        defaults=("${SEC_CORE_DEFAULT_SKILLS[@]}")
    fi

    if [ -n "$manifest" ] && [ -f "$manifest" ] && command -v jq >/dev/null 2>&1; then
        values="$(jq -r --arg target "$target" \
            '.targets[$target].capabilities.skills[]? // empty' \
            "$manifest" 2>/dev/null || true)"
    fi

    if [ -n "$values" ]; then
        printf '%s\n' "$values"
    else
        printf '%s\n' "${defaults[@]}"
    fi
}
