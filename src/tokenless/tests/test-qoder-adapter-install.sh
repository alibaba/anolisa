#!/usr/bin/env bash
# test-qoder-adapter-install.sh — Regression test for the qoder adapter installer.
#
# Regression covered: install.sh used to register a symlink to the raw
# plugin directory, so the ${QODER_TOKENLESS_HOOKS} placeholder in
# hooks.json reached the qodercli plugin cache unexpanded. Consumers that
# load the cached hooks.json directly (the Qoder IDE shares ~/.qoder with
# qodercli) never expand that variable, producing broken hook commands
# like `python3 /rewrite_hook.py` whose non-zero exit is treated as a
# tool-call block before the hook's own fail-open can run.
#
# The test sandboxes HOME, stubs qodercli's `plugins install` with the
# real binary's observable behavior (verbatim copy into the plugin
# cache), runs the installer, and asserts on the CACHED hooks.json —
# not the source one.
#
# Usage: bash tests/test-qoder-adapter-install.sh [path-to-install.sh]
# An alternative installer path may be passed to check other revisions,
# e.g. one extracted via `git show HEAD~1:...` — the test must FAIL on
# an installer that registers the raw plugin dir.

set -uo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m'

PASS=0
FAIL=0
log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((PASS++)); }
log_fail() { echo -e "${RED}[FAIL]${NC} $1"; ((FAIL++)); }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ADAPTER_DIR="$(cd "$SCRIPT_DIR/../adapters/tokenless" && pwd)"
INSTALL_SH="${1:-$ADAPTER_DIR/qoder/scripts/install.sh}"

[ -f "$INSTALL_SH" ] || { echo "installer not found: $INSTALL_SH" >&2; exit 1; }

# install.sh hard-requires python3 (settings merge) and the stub uses it
# for plugin.json version parsing. Check up front so a missing interpreter
# fails the test with a clear cause instead of a misleading installer error.
command -v python3 >/dev/null 2>&1 || { echo "python3 is required" >&2; exit 1; }

SANDBOX="$(mktemp -d -t tokenless-qoder-install-test.XXXXXX)"
trap 'rm -rf "$SANDBOX"' EXIT

FAKE_HOME="$SANDBOX/home"
mkdir -p "$FAKE_HOME/.qoder/bin/qodercli"

# Stub qodercli: emulate `plugins install <dir>` with the real binary's
# observable behavior — copy the plugin verbatim into the cache, named
# after the directory, versioned by .qoder-plugin/plugin.json.
cat > "$FAKE_HOME/.qoder/bin/qodercli/qodercli" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [ "${1:-}" = "plugins" ] && [ "${2:-}" = "install" ]; then
    src="${3:?missing plugin dir}"
    name="$(basename "$src")"
    version="0.0.0"
    pj="$src/.qoder-plugin/plugin.json"
    if [ -f "$pj" ]; then
        version="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1])).get('version','0.0.0'))" "$pj" 2>/dev/null || echo "0.0.0")"
    fi
    dest="$HOME/.qoder/plugins/cache/local/$name/$version"
    mkdir -p "$dest"
    cp -R "$src"/. "$dest/"
    echo "Installed plugin $name@$version"
    exit 0
fi
echo "stub qodercli: unsupported command: $*" >&2
exit 1
EOF
chmod +x "$FAKE_HOME/.qoder/bin/qodercli/qodercli"

log_info "Running installer under test: $INSTALL_SH"
install_out="$(HOME="$FAKE_HOME" ANOLISA_ADAPTER_DIR="$ADAPTER_DIR" bash "$INSTALL_SH" 2>&1)"
install_rc=$?
echo "$install_out" | sed 's/^/    /'
if [ $install_rc -eq 0 ]; then
    log_pass "installer exits 0 with stub qodercli"
else
    log_fail "installer exited $install_rc"
    echo ""
    echo "Summary: $PASS passed, $FAIL failed"
    exit 1
fi

CACHE_HOOKS="$(ls -d "$FAKE_HOME"/.qoder/plugins/cache/local/tokenless/*/hooks.json 2>/dev/null | head -1 || true)"

# 1. Plugin cache was populated.
if [ -n "$CACHE_HOOKS" ] && [ -f "$CACHE_HOOKS" ]; then
    log_pass "plugin cache populated with hooks.json"
else
    log_fail "plugin cache hooks.json missing"
fi

# 2. REGRESSION: cached hooks.json must not retain the placeholder.
if [ -n "$CACHE_HOOKS" ] && grep -q 'QODER_TOKENLESS_HOOKS' "$CACHE_HOOKS"; then
    log_fail 'cached hooks.json still contains ${QODER_TOKENLESS_HOOKS} placeholder'
else
    log_pass "cached hooks.json has no unexpanded placeholder"
fi

# 3. Cached hooks.json references the absolute hooks dir.
if [ -n "$CACHE_HOOKS" ] && grep -qF "$ADAPTER_DIR/common/hooks" "$CACHE_HOOKS"; then
    log_pass "cached hooks.json uses absolute hooks path"
else
    log_fail "cached hooks.json missing absolute path: $ADAPTER_DIR/common/hooks"
fi

# 4. Every script referenced by a cached hook command exists on disk.
if [ -n "$CACHE_HOOKS" ]; then
    missing="$(CACHE_HOOKS="$CACHE_HOOKS" python3 - <<'PYEOF'
import json, os, shlex
cfg = json.load(open(os.environ['CACHE_HOOKS']))
missing = []
for entries in cfg.get('hooks', {}).values():
    for entry in entries:
        for hook in entry.get('hooks') or []:
            parts = shlex.split(hook.get('command', ''))
            if parts and not os.path.exists(parts[-1]):
                missing.append(hook['command'])
print('\n'.join(missing))
PYEOF
)"
    if [ -z "$missing" ]; then
        log_pass "all cached hook commands resolve to existing files"
    else
        log_fail "hook commands reference missing files: $missing"
    fi
fi

# 5. settings.json merge still works and also carries absolute paths.
SETTINGS="$FAKE_HOME/.qoder/settings.json"
if [ -f "$SETTINGS" ] && grep -qF "$ADAPTER_DIR/common/hooks" "$SETTINGS"; then
    log_pass "settings.json hooks use absolute hooks path"
else
    log_fail "settings.json missing absolute hook paths"
fi
if [ -f "$SETTINGS" ] && grep -qF 'tokenless@local' "$SETTINGS"; then
    log_pass "settings.json enables tokenless@local plugin"
else
    log_fail "settings.json missing plugins.enabled entry"
fi

echo ""
echo "============================================"
echo "  Summary: $PASS passed, $FAIL failed"
echo "============================================"
[ "$FAIL" -gt 0 ] && exit 1
echo -e "\n${GREEN}All tests passed!${NC}"
