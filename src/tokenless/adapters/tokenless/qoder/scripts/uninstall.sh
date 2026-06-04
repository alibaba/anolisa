#!/usr/bin/env bash
# uninstall.sh — Remove tokenless plugin from Qoder CLI.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-qoder}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
SETTINGS_PATH="$HOME/.qoder/settings.json"

# Find qodercli binary. Mirrors install.sh: highest versioned binary
# first (sort -V), then unversioned, then PATH.
QODERCLI=""
versioned_glob="$HOME/.qoder/bin/qodercli/qodercli-${ANOLISA_QODER_VERSION:-*}"
# shellcheck disable=SC2086  # intentional glob expansion
latest_versioned="$(ls -d $versioned_glob 2>/dev/null | sort -V | tail -1 || true)"

for candidate in "$latest_versioned" \
                 "$HOME/.qoder/bin/qodercli/qodercli" \
                 "qodercli"; do
    [ -z "$candidate" ] && continue
    if [ -x "$candidate" ] || command -v "$candidate" &>/dev/null; then
        QODERCLI="$candidate"
        break
    fi
done

echo "[${COMPONENT}] Removing ${AGENT} plugin..."

# Unregister plugin via qodercli standard command
if [ -n "$QODERCLI" ]; then
    "$QODERCLI" plugins uninstall tokenless || true
else
    echo "[${COMPONENT}] WARNING: qodercli not found, cannot unregister plugin"
fi

# Remove hooks from settings.json. Match by hook "name" prefix "tokenless-"
# rather than substring search in "command" — the latter would also nuke
# any user-defined hook whose command merely mentions tokenless.
if [ ! -f "$SETTINGS_PATH" ]; then
    echo "[${COMPONENT}] ${AGENT} plugin removed."
    exit 0
fi

if ! command -v python3 &>/dev/null; then
    echo "[${COMPONENT}] WARNING: python3 not found, cannot prune hooks from ${SETTINGS_PATH}" >&2
    echo "[${COMPONENT}] ${AGENT} plugin removed (hooks left in place)."
    exit 0
fi

if ! SETTINGS_PATH="$SETTINGS_PATH" COMPONENT="$COMPONENT" python3 - <<'PYEOF'
import json, os, sys

settings_path = os.environ['SETTINGS_PATH']
component = os.environ.get('COMPONENT', 'tokenless')

try:
    with open(settings_path) as f:
        cfg = json.load(f)
except (OSError, json.JSONDecodeError) as e:
    print(f'[{component}] WARNING: cannot parse {settings_path}: {e}', file=sys.stderr)
    sys.exit(1)

hooks = cfg.get('hooks', {})
if not isinstance(hooks, dict):
    sys.exit(0)

removed = False
for event in list(hooks.keys()):
    keep = []
    for entry in hooks[event]:
        owned = any(
            (h.get('name') or '').startswith('tokenless-')
            for h in (entry.get('hooks') or [])
        )
        if owned:
            removed = True
        else:
            keep.append(entry)
    if keep:
        hooks[event] = keep
    else:
        del hooks[event]
        removed = True

# Remove tokenless@local from plugins.enabled — install.sh writes it
# because qodercli plugins install does not touch settings.json's
# plugins.enabled. Clean up our own state unconditionally: must run
# regardless of remaining user hooks, and even when no tokenless hook
# entries needed pruning (prior partial uninstall, manual edit).
plugins_cfg = cfg.get('plugins')
if isinstance(plugins_cfg, dict):
    enabled = plugins_cfg.get('enabled')
    if isinstance(enabled, list):
        plugin_id = 'tokenless@local'
        if plugin_id in enabled:
            enabled.remove(plugin_id)
            removed = True
        if not enabled:
            plugins_cfg.pop('enabled', None)
    if not plugins_cfg:
        cfg.pop('plugins', None)

if not removed:
    sys.exit(0)

if hooks:
    cfg['hooks'] = hooks
else:
    cfg.pop('hooks', None)

# Atomic write
tmp_path = settings_path + '.tmp'
with open(tmp_path, 'w') as f:
    json.dump(cfg, f, indent=2)
os.replace(tmp_path, settings_path)
print(f'[{component}] Removed tokenless hooks from settings.json')
PYEOF
then
    echo "[${COMPONENT}] WARNING: failed to update ${SETTINGS_PATH}" >&2
fi

echo "[${COMPONENT}] ${AGENT} plugin removed."
