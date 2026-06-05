#!/usr/bin/env bash
# _common.sh — Shared helpers for codex adapter scripts.
# Source this file from install.sh and uninstall.sh.

# Resolve the codex CLI binary.
resolve_codex() {
    if [[ -n "$CODEX_BIN" ]] && command -v "$CODEX_BIN" &>/dev/null; then
        echo "$CODEX_BIN"
        return
    fi
    for candidate in codex /usr/local/bin/codex /usr/bin/codex "$HOME/.local/bin/codex"; do
        if command -v "$candidate" &>/dev/null; then
            echo "$candidate"
            return
        fi
    done
    # Last resort: direct path check
    for candidate in /usr/local/bin/codex /usr/bin/codex "$HOME/.local/bin/codex"; do
        if [[ -x "$candidate" ]]; then
            echo "$candidate"
            return
        fi
    done
    echo ""
}
