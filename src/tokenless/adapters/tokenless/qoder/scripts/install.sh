#!/usr/bin/env bash
# install.sh — Install tokenless plugin for Qoder CLI.
#
# Responsibility boundary:
#   - Register the plugin with qodercli (`qodercli plugins install`).
#   - Merge our hook commands into ~/.qoder/settings.json under .hooks,
#     dedup by command string so re-installs are idempotent.
#   - Hook scripts themselves live in adapters/tokenless/common/hooks/
#     (shared with cosh/openclaw/hermes); we only inject absolute paths.
set -euo pipefail

AGENT="${ANOLISA_TARGET:-qoder}"
COMPONENT="${ANOLISA_COMPONENT:-tokenless}"
ADAPTER_DIR="${ANOLISA_ADAPTER_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"

PLUGIN_DIR="$ADAPTER_DIR/qoder"
HOOKS_DIR="$ADAPTER_DIR/common/hooks"
SETTINGS_PATH="$HOME/.qoder/settings.json"

# Read version from plugin.json (already templated with the real version
# by the build system). Falls back to "0.0.0" only when plugin.json is
# absent or unparseable.
VERSION="0.0.0"
plugin_json="$PLUGIN_DIR/.qoder-plugin/plugin.json"
if [ -f "$plugin_json" ] && command -v python3 &>/dev/null; then
    VERSION="$(PLUGIN_JSON="$plugin_json" python3 -c "
import json, os
print(json.load(open(os.environ['PLUGIN_JSON'])).get('version','0.0.0'))
" 2>/dev/null || echo "0.0.0")"
elif [ -f "$plugin_json" ] && command -v jq &>/dev/null; then
    VERSION="$(jq -r '.version // "0.0.0"' "$plugin_json" 2>/dev/null || echo "0.0.0")"
fi

# Find qodercli binary. Pick the highest versioned qodercli-X.Y.Z first
# (sort -V handles semver: 10 > 9), then fall back to the unversioned
# binary and finally PATH lookup.
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

if [ -z "$QODERCLI" ]; then
    echo "[${COMPONENT}] ERROR: qodercli not found, aborting plugin registration" >&2
    echo "    Install Qoder CLI first: https://qoder.com/cli" >&2
    exit 1
fi

# python3 is required to merge hooks into settings.json; without it the
# `qodercli plugins install` step would succeed but no hooks would fire,
# silently leaving tokenless inactive. Fail loudly instead.
if ! command -v python3 &>/dev/null; then
    echo "[${COMPONENT}] ERROR: python3 required to merge hooks into ${SETTINGS_PATH}" >&2
    exit 1
fi

echo "[${COMPONENT}] Installing ${AGENT} plugin v${VERSION}..."

# Register plugin via qodercli standard command.
# qodercli derives plugin name from the directory name, so expose the adapter
# under a name matching plugin.json's "name" field via a private tempdir.
# (A predictable /tmp/tokenless would collide across users on shared hosts and
# race with concurrent installs.)
echo "[${COMPONENT}] Registering plugin with qodercli..."
TEMP_DIR="$(mktemp -d -t tokenless-qoder-install.XXXXXX)"
trap 'rm -rf "$TEMP_DIR"' EXIT
ln -sfn "$PLUGIN_DIR" "$TEMP_DIR/tokenless"
# Use `if !` so the failure branch survives `set -e`: a bare
# `OUT=$(...)` assignment would otherwise abort the script before $?
# is captured, and qodercli's stderr (now redirected into the var)
# would never be printed.
if ! PLUGIN_INSTALL_OUT="$("$QODERCLI" plugins install "$TEMP_DIR/tokenless" 2>&1)"; then
    echo "[${COMPONENT}] ERROR: qodercli plugins install failed" >&2
    echo "    Output: $PLUGIN_INSTALL_OUT" >&2
    exit 1
fi
echo "$PLUGIN_INSTALL_OUT"

# Verify the plugin was registered: qodercli plugins list may not show
# freshly installed plugins (qodercli display quirk), so we check the
# cache directory that plugins install should have populated instead.
# qodercli has been observed using two names for the cache key — the
# plugin id ("tokenless") and a target-suffixed variant
# ("tokenless-qoder"); accept either to stay aligned with the %preun
# cleanup in tokenless.spec.in.
CACHE_BASE="$HOME/.qoder/plugins/cache/local"
if [ ! -d "$CACHE_BASE/tokenless" ] && [ ! -d "$CACHE_BASE/tokenless-qoder" ]; then
    echo "[${COMPONENT}] ERROR: qodercli did not create plugin cache under $CACHE_BASE" >&2
    echo "    Inspect with: ${QODERCLI} plugins list" >&2
    exit 1
fi

# Merge hooks into ~/.qoder/settings.json.
# hooks.json carries the ${QODER_TOKENLESS_HOOKS} placeholder; we expand it
# to the actual common/hooks/ path so qodercli sees absolute paths.
HOOKS_DIR="$HOOKS_DIR" \
HOOKS_JSON_PATH="$PLUGIN_DIR/hooks.json" \
SETTINGS_PATH="$SETTINGS_PATH" \
COMPONENT="$COMPONENT" \
python3 - <<'PYEOF'
import json, os, sys

hooks_dir = os.environ['HOOKS_DIR']
hooks_json_path = os.environ['HOOKS_JSON_PATH']
settings_path = os.environ['SETTINGS_PATH']
component = os.environ.get('COMPONENT', 'tokenless')

with open(hooks_json_path) as f:
    hooks_str = f.read()
hooks_str = hooks_str.replace('${QODER_TOKENLESS_HOOKS}', hooks_dir)
resolved = json.loads(hooks_str)

cfg = {}
if os.path.exists(settings_path):
    try:
        with open(settings_path) as f:
            cfg = json.load(f)
    except (OSError, json.JSONDecodeError) as e:
        # Refuse to clobber a settings.json we cannot parse — overwriting
        # with our hooks alone would silently wipe every other user setting.
        print(f'[{component}] ERROR: cannot parse {settings_path}: {e}', file=sys.stderr)
        sys.exit(1)
if not isinstance(cfg, dict):
    cfg = {}

existing_hooks = cfg.get('hooks', {})
if not isinstance(existing_hooks, dict):
    existing_hooks = {}

for event, entries in resolved['hooks'].items():
    existing = existing_hooks.get(event, [])
    existing_names = set()
    for e in existing:
        for h in (e.get('hooks') or []):
            if h.get('name'):
                existing_names.add(h['name'])
    for entry in entries:
        entry_name = None
        for h in (entry.get('hooks') or []):
            if h.get('name'):
                entry_name = h['name']
                break
        if entry_name and entry_name not in existing_names:
            existing.append(entry)
    existing_hooks[event] = existing

cfg['hooks'] = existing_hooks

# Also ensure the plugin is enabled in settings.json. qodercli plugins
# install writes installed_plugins.json but does NOT touch settings.json's
# plugins.enabled — without this the plugin won't show in `plugin list`.
plugins_cfg = cfg.get('plugins')
if not isinstance(plugins_cfg, dict):
    plugins_cfg = {}
    cfg['plugins'] = plugins_cfg
enabled = plugins_cfg.get('enabled')
if not isinstance(enabled, list):
    enabled = []
    plugins_cfg['enabled'] = enabled
plugin_id = 'tokenless@local'
if plugin_id not in enabled:
    enabled.append(plugin_id)
os.makedirs(os.path.dirname(settings_path), exist_ok=True)

# Atomic write: stage to .tmp then os.replace() so a kill mid-write cannot
# leave qodercli with a truncated settings.json.
tmp_path = settings_path + '.tmp'
with open(tmp_path, 'w') as f:
    json.dump(cfg, f, indent=2)
os.replace(tmp_path, settings_path)
print(f'[{component}] Updated {settings_path}')
PYEOF

echo "[${COMPONENT}] ${AGENT} plugin v${VERSION} installed and activated."
