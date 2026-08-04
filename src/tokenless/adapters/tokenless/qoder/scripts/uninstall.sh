#!/usr/bin/env bash
# Uninstall tokenless through Qoder's native plugin lifecycle.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-qoder}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
SCRIPT_DIR="$ADAPTER_DIR/qoder/scripts"
QODER_ROOT="${QODER_CONFIG_DIR:-$HOME/.qoder}"
# Every legacy tokenless writer ignored QODER_CONFIG_DIR and wrote this exact
# historical path. Native Qoder state still uses QODER_ROOT above.
LEGACY_SETTINGS="$HOME/.qoder/settings.json"
USER_DATA_ROOT="${XDG_DATA_HOME:-$HOME/.local/share}"
[[ "$USER_DATA_ROOT" = /* ]] || USER_DATA_ROOT="$HOME/.local/share"
LEGACY_ROOT_ARGS=(
    --legacy-hooks-root "$ADAPTER_DIR/common/hooks"
    --legacy-hooks-root "$USER_DATA_ROOT/anolisa/adapters/tokenless/common/hooks"
    --legacy-hooks-root "/usr/local/share/anolisa/adapters/tokenless/common/hooks"
    --legacy-hooks-root "/usr/share/anolisa/adapters/tokenless/common/hooks"
)

resolve_qodercli() {
    local candidate latest versioned_glob
    versioned_glob="$QODER_ROOT/bin/qodercli/qodercli-${ANOLISA_QODER_VERSION:-*}"
    # shellcheck disable=SC2086 # intentional versioned binary glob
    latest="$(ls -d $versioned_glob 2>/dev/null | sort -V | tail -1 || true)"
    for candidate in "${QODERCLI_BIN:-}" "$latest" \
        "$QODER_ROOT/bin/qodercli/qodercli" qodercli; do
        [ -n "$candidate" ] || continue
        if [ -x "$candidate" ] || command -v "$candidate" >/dev/null 2>&1; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

legacy_cleanup_ok=1
if command -v python3 >/dev/null 2>&1; then
    if migration_out="$(python3 "$SCRIPT_DIR/migrate-legacy-settings.py" \
        "${LEGACY_ROOT_ARGS[@]}" "$LEGACY_SETTINGS" 2>&1)"; then
        printf '[%s] Legacy migration: %s\n' "$COMPONENT" "$migration_out"
    else
        legacy_cleanup_ok=0
        echo "[${COMPONENT}] ERROR: $migration_out" >&2
    fi
else
    legacy_cleanup_ok=0
    echo "[${COMPONENT}] WARNING: python3 unavailable; legacy settings were not inspected" >&2
fi

QODERCLI="$(resolve_qodercli || true)"
if [ -z "$QODERCLI" ]; then
    echo "[${COMPONENT}] ERROR: qodercli not found; native plugin registration is unchanged" >&2
    [ "$legacy_cleanup_ok" -eq 1 ] \
        && echo "[${COMPONENT}] Exact legacy settings cleanup completed before the failure." >&2
    exit 1
fi

echo "[${COMPONENT}] Removing ${AGENT} native plugin..."
if ! uninstall_out="$("$QODERCLI" plugins uninstall tokenless --scope user 2>&1)"; then
    if ! command -v python3 >/dev/null 2>&1; then
        echo "[${COMPONENT}] ERROR: qodercli plugins uninstall failed" >&2
        echo "    $uninstall_out" >&2
        echo "    python3 is required to verify whether the plugin is already absent" >&2
        exit 1
    fi
    if ! list_out="$("$QODERCLI" plugins list --json)" \
        || ! printf '%s' "$list_out" | python3 -c 'import json,sys; json.load(sys.stdin)' \
            >/dev/null 2>&1; then
        echo "[${COMPONENT}] ERROR: qodercli plugins uninstall failed and plugin inventory is unavailable" >&2
        echo "    $uninstall_out" >&2
        exit 1
    fi
    if printf '%s' "$list_out" | python3 "$SCRIPT_DIR/verify-plugin-list.py" \
        --presence-only >/dev/null 2>&1; then
        echo "[${COMPONENT}] ERROR: qodercli plugins uninstall failed and tokenless@local is still registered" >&2
        echo "    $uninstall_out" >&2
        exit 1
    else
        presence_rc=$?
        if [ "$presence_rc" -ne 1 ]; then
            echo "[${COMPONENT}] ERROR: qodercli plugins uninstall failed and plugin inventory has an unrecognized shape" >&2
            echo "    $uninstall_out" >&2
            exit 1
        fi
    fi
    echo "[${COMPONENT}] Native plugin was already absent; continuing legacy cleanup."
else
    printf '%s\n' "$uninstall_out"
fi

if [ "$legacy_cleanup_ok" -ne 1 ]; then
    echo "[${COMPONENT}] ERROR: native plugin removed, but legacy settings cleanup is incomplete" >&2
    exit 1
fi

echo "[${COMPONENT}] ${AGENT} plugin removed. Restart Qoder or run /plugins reload."
