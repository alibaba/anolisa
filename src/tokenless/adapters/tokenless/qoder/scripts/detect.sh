#!/usr/bin/env bash
# Detect the native Qoder plugin API required by tokenless.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-qoder}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
PLUGIN_DIR="$ADAPTER_DIR/qoder"
QODER_ROOT="${QODER_CONFIG_DIR:-$HOME/.qoder}"

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

QODERCLI="$(resolve_qodercli || true)"
if [ -z "$QODERCLI" ]; then
    echo "[${COMPONENT}] ${AGENT}: qodercli not found" >&2
    exit 1
fi
if ! command -v python3 >/dev/null 2>&1; then
    echo "[${COMPONENT}] ${AGENT}: python3 is required for safe plugin verification" >&2
    exit 1
fi

for subcommand in validate install list uninstall; do
    if ! "$QODERCLI" plugins "$subcommand" --help >/dev/null 2>&1; then
        echo "[${COMPONENT}] ${AGENT}: Qoder lacks 'plugins ${subcommand}'; upgrade Qoder" >&2
        exit 1
    fi
done

if ! list_json="$("$QODERCLI" plugins list --json 2>/dev/null)" \
    || ! LIST_JSON="$list_json" python3 -c \
        'import json, os; json.loads(os.environ["LIST_JSON"])' 2>/dev/null; then
    echo "[${COMPONENT}] ${AGENT}: Qoder lacks JSON plugin inventory; upgrade Qoder" >&2
    exit 1
fi

if ! validate_out="$("$QODERCLI" plugins validate "$PLUGIN_DIR" 2>&1)"; then
    echo "[${COMPONENT}] ${AGENT}: native plugin validation failed; upgrade Qoder" >&2
    echo "    $validate_out" >&2
    exit 1
fi

echo "[${COMPONENT}] ${AGENT}: native plugin API detected at ${QODERCLI}"
