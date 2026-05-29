#!/usr/bin/env bash
# run-hook.sh — Locate and exec a shared tokenless hook script.
#
# Claude Code copies the plugin into a versioned cache directory
# (~/.claude/plugins/cache/<mkt>/<plugin>/<ver>/), so relative paths into
# common/ no longer resolve. We instead look up the named hook under the
# FHS layout installed by the tokenless RPM / Makefile.
#
# Usage:    run-hook.sh <hook-script-basename> [args...]
# Examples: run-hook.sh rewrite_hook.py
#           run-hook.sh compress_response_hook.py
#           run-hook.sh tool_ready_hook.sh
#
# Fail-open contract: any not-found / missing-interpreter condition emits
# an empty JSON object on stdout and exits 0, so Claude Code never blocks
# on us.
#
# PreToolUse matcher overlap is by design: hooks.json registers both a
# Bash-specific entry (rewrite) and a catch-all entry (tool-ready). Bash
# tool calls therefore fire both — Claude Code evaluates each matching
# matcher independently, so this is the documented way to attach a
# tool-specific hook alongside a global one. The timeout values are
# upper bounds; observed runtimes are well under them.
set -euo pipefail

SCRIPT="${1:?usage: run-hook.sh <hook-script-basename> [args...]}"
shift

fail_open() { echo "{}"; exit 0; }

# Reject anything but a bare basename. Practical risk is low — hooks.json is
# RPM-installed root:root 0644 and unwritable at runtime — but a defense-in-
# depth check costs one line and stops any future caller from smuggling a
# traversal segment through this argument.
case "$SCRIPT" in
    */*|"") fail_open ;;
esac

CANDIDATES=(
    "/usr/share/anolisa/adapters/tokenless/common/hooks/${SCRIPT}"
    "${HOME}/.local/share/anolisa/adapters/tokenless/common/hooks/${SCRIPT}"
)

for candidate in "${CANDIDATES[@]}"; do
    [ -f "$candidate" ] || continue
    case "$candidate" in
        *.py)
            command -v python3 >/dev/null 2>&1 || fail_open
            exec python3 "$candidate" "$@"
            ;;
        *.sh)
            exec bash "$candidate" "$@"
            ;;
        *)
            fail_open
            ;;
    esac
done

fail_open
