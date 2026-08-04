#!/usr/bin/env bash
# Install tokenless through Qoder's native plugin lifecycle.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-qoder}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
PLUGIN_DIR="$ADAPTER_DIR/qoder"
SCRIPT_DIR="$PLUGIN_DIR/scripts"
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

fail_upgrade() {
    echo "[${COMPONENT}] ERROR: $1" >&2
    echo "    Upgrade Qoder to a release with native plugin hooks and JSON inventory." >&2
    exit 1
}

QODERCLI="$(resolve_qodercli || true)"
[ -n "$QODERCLI" ] || fail_upgrade "qodercli not found"
command -v python3 >/dev/null 2>&1 \
    || fail_upgrade "python3 is required for safe plugin verification"

for required in \
    "$PLUGIN_DIR/.qoder-plugin/plugin.json" \
    "$PLUGIN_DIR/hooks/hooks.json" \
    "$PLUGIN_DIR/hooks/run-hook.sh" \
    "$PLUGIN_DIR/common/hooks/hook_utils.py" \
    "$PLUGIN_DIR/common/hooks/tool_ready_hook.sh" \
    "$PLUGIN_DIR/common/hooks/rewrite_hook.py" \
    "$PLUGIN_DIR/common/hooks/compress_response_hook.py" \
    "$PLUGIN_DIR/common/tool-ready-spec.json" \
    "$PLUGIN_DIR/common/tokenless-env-fix.sh" \
    "$PLUGIN_DIR/commands/tokenless-stats.md"; do
    [ -e "$required" ] || fail_upgrade "incomplete Qoder plugin bundle: missing $required"
done

for subcommand in validate install list uninstall; do
    "$QODERCLI" plugins "$subcommand" --help >/dev/null 2>&1 \
        || fail_upgrade "Qoder lacks 'plugins ${subcommand}'"
done

list_plugins() {
    "$QODERCLI" plugins list --json
}

preexisting=0
if before_json="$(list_plugins)"; then
    if ! printf '%s' "$before_json" | python3 -c 'import json,sys; json.load(sys.stdin)' \
        >/dev/null 2>&1; then
        fail_upgrade "'plugins list --json' returned invalid JSON"
    fi
    if printf '%s' "$before_json" | python3 "$SCRIPT_DIR/verify-plugin-list.py" \
        --presence-only >/dev/null 2>&1; then
        preexisting=1
    else
        presence_rc=$?
        [ "$presence_rc" -eq 1 ] \
            || fail_upgrade "'plugins list --json' returned an unrecognized inventory structure"
    fi
else
    fail_upgrade "Qoder does not support 'plugins list --json'"
fi

if ! validate_out="$("$QODERCLI" plugins validate "$PLUGIN_DIR" 2>&1)"; then
    fail_upgrade "Qoder rejected the native tokenless plugin: $validate_out"
fi

echo "[${COMPONENT}] Installing ${AGENT} as a native Qoder plugin..."
if ! install_out="$("$QODERCLI" plugins install "$PLUGIN_DIR" --scope user 2>&1)"; then
    echo "[${COMPONENT}] ERROR: qodercli plugins install failed" >&2
    echo "    $install_out" >&2
    exit 1
fi
printf '%s\n' "$install_out"

rollback() {
    if [ "$preexisting" -eq 1 ]; then
        echo "[${COMPONENT}] WARNING: pre-existing native plugin retained; inspect with '$QODERCLI plugins list'" >&2
        return
    fi
    if ! "$QODERCLI" plugins uninstall tokenless --scope user >/dev/null 2>&1; then
        echo "[${COMPONENT}] DEGRADED: rollback failed; run '$QODERCLI plugins uninstall tokenless --scope user'" >&2
    fi
}

if ! after_json="$(list_plugins)" \
    || ! printf '%s' "$after_json" | python3 "$SCRIPT_DIR/verify-plugin-list.py"; then
    echo "[${COMPONENT}] ERROR: Qoder did not activate all tokenless plugin resources" >&2
    rollback
    exit 1
fi

if ! migration_out="$(python3 "$SCRIPT_DIR/migrate-legacy-settings.py" \
    "${LEGACY_ROOT_ARGS[@]}" "$LEGACY_SETTINGS" 2>&1)"; then
    echo "[${COMPONENT}] ERROR: $migration_out" >&2
    rollback
    exit 1
fi
printf '[%s] Legacy migration: %s\n' "$COMPONENT" "$migration_out"

echo "[${COMPONENT}] ${AGENT} plugin installed and verified as tokenless@local."
echo "[${COMPONENT}] Run /plugins reload in Qoder or restart Qoder to apply it."
