#!/usr/bin/env bash
# PostToolUse replacement semantics test (issue #1645)
#
# Verifies compress_response_hook.py output contracts per agent:
#   - claude-code >= 2.1.121: compressed payload replaces the tool result
#     via hookSpecificOutput.updatedToolOutput (present exactly once, original
#     sentinel absent, built-in tool schema preserved)
#   - claude-code with undetectable/old version: fail open (pass-through,
#     no duplicate injection)
#   - non-beneficial compression: pass-through for every agent
#   - other agents: additionalContext contract unchanged
#
# Uses stub `tokenless` / `claude` binaries so no real installation is needed.
# JSON handling uses python3 only (already required by the hook itself), so
# the test carries no extra dependency such as jq.

set -uo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

PASS=0
FAIL=0
TOTAL=0

pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((PASS++)); ((TOTAL++)); }
fail() { echo -e "${RED}[FAIL]${NC} $1"; ((FAIL++)); ((TOTAL++)); }
info() { echo -e "${BLUE}[INFO]${NC} $1"; }
section() { echo -e "\n${YELLOW}========== $1 ==========${NC}\n"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HOOK_DIR="$SCRIPT_DIR/../adapters/tokenless/common/hooks"
HOOK="$HOOK_DIR/compress_response_hook.py"

if ! command -v python3 >/dev/null 2>&1; then
    echo -e "${RED}ERROR: python3 not found (required by the hook itself)${NC}"; exit 1
fi
[ -f "$HOOK" ] || { echo -e "${RED}ERROR: hook not found: $HOOK${NC}"; exit 1; }

WORKDIR=$(mktemp -d /tmp/tokenless_replace_test_XXXXXX)
trap 'rm -rf "$WORKDIR"' EXIT

# -- JSON helpers (python3-based, no jq dependency) -----------------------------

jget() {
    # $1 = JSON document; $2 = dot-separated key path. Prints empty when the
    # path is missing; dict/list values are re-serialized as compact JSON.
    python3 -c '
import json, sys
try:
    obj = json.loads(sys.argv[1])
except Exception:
    print(); sys.exit(0)
for key in sys.argv[2].split("."):
    if isinstance(obj, dict) and key in obj:
        obj = obj[key]
    else:
        print(); sys.exit(0)
if isinstance(obj, (dict, list)):
    print(json.dumps(obj, ensure_ascii=False))
elif isinstance(obj, bool):
    print("true" if obj else "false")
else:
    print(obj)
' "$1" "$2"
}

build_bash_input() {
    # $1 = tool_use_id; $2 = stdout; $3 = stderr; $4 = exit_code ("" to omit)
    python3 -c '
import json, sys
resp = {"stdout": sys.argv[2], "stderr": sys.argv[3],
        "interrupted": False, "isImage": False}
if sys.argv[4]:
    resp["exit_code"] = int(sys.argv[4])
print(json.dumps({"tool_name": "Bash", "session_id": "sess-1",
                  "tool_use_id": sys.argv[1], "tool_response": resp}))
' "$1" "$2" "$3" "$4"
}

# -- stub binaries -------------------------------------------------------------

make_stub_tokenless() {
    # $1 = dir; $2 = mode (compress|passthrough)
    local dir="$1" mode="$2"
    if [ "$mode" = "compress" ]; then
        cat > "$dir/tokenless" <<'STUB'
#!/usr/bin/env bash
case "${1:-}" in
  compress-response)
    python3 -c '
import json, sys
data = json.load(sys.stdin)
def shrink(v):
    return v[:100] if isinstance(v, str) else v
if isinstance(data, dict):
    out = {k: shrink(v) for k, v in data.items() if v not in (None, "", {}, [])}
else:
    out = data
print(json.dumps(out, separators=(",", ":")))
'
    ;;
  compress-toon) exit 0 ;;
  *) exit 0 ;;
esac
STUB
    else
        # passthrough: compression yields no savings
        cat > "$dir/tokenless" <<'STUB'
#!/usr/bin/env bash
case "${1:-}" in
  compress-response) cat ;;
  compress-toon) exit 0 ;;
  *) exit 0 ;;
esac
STUB
    fi
    chmod +x "$dir/tokenless"
}

make_stub_claude() {
    # $1 = dir; $2 = version string or "broken"
    local dir="$1" ver="$2"
    if [ "$ver" = "broken" ]; then
        printf '#!/usr/bin/env bash\nexit 1\n' > "$dir/claude"
    else
        printf '#!/usr/bin/env bash\necho "%s (Claude Code)"\n' "$ver" > "$dir/claude"
    fi
    chmod +x "$dir/claude"
}

run_hook() {
    # $1 = agent id; $2 = stub dir; $3 = input JSON
    local agent="$1" stubdir="$2" input="$3" home
    home=$(mktemp -d "$WORKDIR/home_XXXXXX")
    echo "$input" | HOME="$home" PATH="$stubdir:$PATH" \
        TOKENLESS_AGENT_ID="$agent" python3 "$HOOK" 2>/dev/null
}

# Sentinel long enough (>200 chars) to enter the compression pipeline and be
# visibly truncated by the stub (100-char cap).
SENTINEL="TOKENLESS_SENTINEL_$(head -c8 /dev/urandom | od -An -tx1 | tr -d ' \n')"
LONG_OUT="${SENTINEL}_$(printf 'x%.0s' $(seq 1 400))"

BASH_INPUT=$(build_bash_input "toolu_1" "$LONG_OUT" "" "")

# ===== 1. claude-code >= 2.1.121: replacement via updatedToolOutput =====
section "Test 1: claude-code replacement semantics"

STUB1="$WORKDIR/stub1"; mkdir -p "$STUB1"
make_stub_tokenless "$STUB1" compress
make_stub_claude "$STUB1" "2.1.210"

out=$(run_hook claude-code "$STUB1" "$BASH_INPUT")

info "1.1: updatedToolOutput present with compressed stdout"
updated_stdout=$(jget "$out" "hookSpecificOutput.updatedToolOutput.stdout")
case "$updated_stdout" in
    "$SENTINEL"*) pass "updatedToolOutput.stdout carries compressed content" ;;
    *) fail "updatedToolOutput.stdout missing or wrong: $out" ;;
esac

info "1.2: original (uncompressed) sentinel payload absent"
if echo "$out" | grep -qF "$LONG_OUT"; then
    fail "full original payload leaked into hook output"
else
    pass "original full payload absent from hook output"
fi

info "1.3: compressed payload present exactly once"
count=$(echo "$out" | grep -oF "$SENTINEL" | wc -l)
[ "$count" -eq 1 ] && pass "sentinel appears exactly once" \
    || fail "sentinel appears $count times (expected 1)"

info "1.4: compressed payload not duplicated into additionalContext"
extra=$(jget "$out" "hookSpecificOutput.additionalContext")
if echo "$extra" | grep -qF "$SENTINEL"; then
    fail "compressed payload duplicated in additionalContext"
else
    pass "additionalContext free of compressed payload"
fi

info "1.5: built-in Bash output schema preserved"
schema_ok=$(python3 -c '
import json, sys
try:
    uto = json.loads(sys.argv[1])["hookSpecificOutput"]["updatedToolOutput"]
except Exception:
    print("false"); sys.exit(0)
keys = {"stdout", "stderr", "interrupted", "isImage"}
print("true" if isinstance(uto, dict) and keys <= set(uto) else "false")
' "$out")
[ "$schema_ok" = "true" ] && pass "stdout/stderr/interrupted/isImage all present" \
    || fail "schema fields missing: $out"

# ===== 2. Version gating: fail open =====
section "Test 2: version gating (fail open)"

info "2.1: old Claude Code (2.1.100) → pass-through, no injection"
STUB2="$WORKDIR/stub2"; mkdir -p "$STUB2"
make_stub_tokenless "$STUB2" compress
make_stub_claude "$STUB2" "2.1.100"
out=$(run_hook claude-code "$STUB2" "$BASH_INPUT")
trimmed=$(echo "$out" | tr -d '[:space:]')
[ "$trimmed" = "{}" ] && pass "old version passes through with {}" \
    || fail "expected {}, got: $out"

info "2.2: undetectable claude version → pass-through"
STUB3="$WORKDIR/stub3"; mkdir -p "$STUB3"
make_stub_tokenless "$STUB3" compress
make_stub_claude "$STUB3" broken
out=$(run_hook claude-code "$STUB3" "$BASH_INPUT")
trimmed=$(echo "$out" | tr -d '[:space:]')
[ "$trimmed" = "{}" ] && pass "unknown version passes through with {}" \
    || fail "expected {}, got: $out"

# ===== 3. Non-beneficial compression: pass-through for all agents =====
section "Test 3: non-beneficial compression"

STUB4="$WORKDIR/stub4"; mkdir -p "$STUB4"
make_stub_tokenless "$STUB4" passthrough
make_stub_claude "$STUB4" "2.1.210"

info "3.1: claude-code — no savings → no additive duplication"
out=$(run_hook claude-code "$STUB4" "$BASH_INPUT")
trimmed=$(echo "$out" | tr -d '[:space:]')
[ "$trimmed" = "{}" ] && pass "claude-code passes through with {}" \
    || fail "expected {}, got: $out"

info "3.2: qoder-cli — no savings → no additive duplication"
out=$(run_hook qoder-cli "$STUB4" "$BASH_INPUT")
trimmed=$(echo "$out" | tr -d '[:space:]')
[ "$trimmed" = "{}" ] && pass "qoder-cli passes through with {}" \
    || fail "expected {}, got: $out"

# ===== 4. Other agents keep the additionalContext contract =====
section "Test 4: non-claude agents unchanged"

info "4.1: qoder-cli — compressed payload via additionalContext"
out=$(run_hook qoder-cli "$STUB1" "$BASH_INPUT")
extra=$(jget "$out" "hookSpecificOutput.additionalContext")
updated=$(jget "$out" "hookSpecificOutput.updatedToolOutput")
if echo "$extra" | grep -qF "$SENTINEL" && [ -z "$updated" ]; then
    pass "qoder-cli keeps additionalContext, no updatedToolOutput"
else
    fail "qoder-cli contract changed: $out"
fi

# ===== 5. Env attribution stays additive-only on the replacement path =====
section "Test 5: env attribution with replacement"

ERR_OUT="bash: frobnicate: command not found. $(printf 'e%.0s' $(seq 1 300)) ${SENTINEL}"
ERR_INPUT=$(build_bash_input "toolu_2" "" "$ERR_OUT" 127)

out=$(run_hook claude-code "$STUB1" "$ERR_INPUT")

info "5.1: updatedToolOutput present for error response"
updated=$(jget "$out" "hookSpecificOutput.updatedToolOutput")
[ -n "$updated" ] && pass "replacement emitted for error response" \
    || fail "no updatedToolOutput: $out"

info "5.2: additionalContext carries only the env attribution"
extra=$(jget "$out" "hookSpecificOutput.additionalContext")
if echo "$extra" | grep -qF "[tokenless:env]" && ! echo "$extra" | grep -qF "$SENTINEL"; then
    pass "additionalContext is attribution-only"
else
    fail "additionalContext wrong: $extra"
fi

# ===== 6. Version cache file is written with hardened permissions =====
section "Test 6: version cache hardening"

info "6.1: cache file mode is 0600 under a 0700 ~/.tokenless"
home=$(mktemp -d "$WORKDIR/home_perm_XXXXXX")
echo "$BASH_INPUT" | HOME="$home" PATH="$STUB1:$PATH" \
    TOKENLESS_AGENT_ID=claude-code python3 "$HOOK" >/dev/null 2>&1
cache="$home/.tokenless/.claude-version"
if [ -f "$cache" ]; then
    dir_mode=$(stat -c '%a' "$home/.tokenless")
    file_mode=$(stat -c '%a' "$cache")
    [ "$file_mode" = "600" ] && [ "$dir_mode" = "700" ] \
        && pass "cache dir=700 file=600" \
        || fail "unexpected modes: dir=$dir_mode file=$file_mode"
else
    fail "version cache not written: $cache"
fi

# ===== summary =====
echo ""
echo "============================================"
echo "  Summary: ${PASS}/${TOTAL} passed"
echo "============================================"
[ "$FAIL" -gt 0 ] && exit 1
echo -e "\n${GREEN}All tests passed!${NC}"
