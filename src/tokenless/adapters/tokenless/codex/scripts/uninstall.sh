#!/usr/bin/env bash
# Uninstall the tokenless Codex plugin and clean up plugin data.
#
# Removes:
#   - Codex plugin registration (codex plugin remove)
#   - Codex marketplace entry (codex plugin marketplace remove)
#   - Marketplace symlink directory
#   - The tokenless binary from the install prefix
#
# NOTE: This script does NOT remove $HOME/.tokenless/ — that directory
# holds the SQLite stats DB and rewrite context shared by every tokenless
# adapter (cosh, qoder, hermes, openclaw, claude-code). Removing it here
# would destroy data belonging to plugins still installed.
#
# Usage:
#   ./uninstall.sh                    # Interactive (asks confirmation)
#   ./uninstall.sh --non-interactive  # CI / automated (no confirmation)

set -euo pipefail

INTERACTIVE=1
if [[ "${1:-}" == "--non-interactive" ]]; then
    INTERACTIVE=0
fi

PREFIX="${TOKENLESS_INSTALL_PREFIX:-$HOME/.local}"
BINDIR="$PREFIX/bin"
CODEX_BIN="${CODEX_BIN:-}"
MARKETPLACE_NAME="anolisa-tokenless"
MARKETPLACE_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/anolisa/codex-marketplace"

# Resolve codex binary via shared helper (see _common.sh).
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=./_common.sh
source "$SCRIPT_DIR/_common.sh"

echo "[tokenless] Codex plugin uninstall"

# 1. Remove codex plugin registration
CODEX_BIN="$(resolve_codex)"
if [[ -n "$CODEX_BIN" ]]; then
    if "$CODEX_BIN" plugin list 2>/dev/null | grep -q "tokenless"; then
        echo "[tokenless] Removing codex plugin 'tokenless@${MARKETPLACE_NAME}'..."
        "$CODEX_BIN" plugin remove "tokenless@${MARKETPLACE_NAME}" 2>&1 || true
    fi
    if "$CODEX_BIN" plugin marketplace list 2>/dev/null | grep -q "^${MARKETPLACE_NAME}[[:space:]]"; then
        echo "[tokenless] Removing marketplace '${MARKETPLACE_NAME}'..."
        "$CODEX_BIN" plugin marketplace remove "$MARKETPLACE_NAME" 2>&1 || true
    fi
else
    echo "[tokenless] codex CLI not found, skipping plugin unregistration."
fi

# 2. Remove marketplace symlink directory
if [[ -d "$MARKETPLACE_DIR" ]]; then
    rm -rf "$MARKETPLACE_DIR"
    echo "[tokenless] Removed marketplace directory: $MARKETPLACE_DIR"
fi

# 3. Remove binary
if [[ -f "$BINDIR/tokenless" ]]; then
    if [[ $INTERACTIVE -eq 1 ]]; then
        read -rp "Remove $BINDIR/tokenless? [y/N] " answer
        if [[ "$answer" =~ ^[Yy]$ ]]; then
            rm -f "$BINDIR/tokenless"
            echo "[tokenless] Removed: $BINDIR/tokenless"
        else
            echo "[tokenless] Skipped: $BINDIR/tokenless"
        fi
    else
        rm -f "$BINDIR/tokenless"
        echo "[tokenless] Removed: $BINDIR/tokenless"
    fi
else
    echo "[tokenless] Binary not found: $BINDIR/tokenless"
fi

echo "[tokenless] Uninstall complete."
