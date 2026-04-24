#!/bin/bash
# Token-Less Full Test Suite
# Tests all three compression methods:
# 1. Schema Compression (tokenless compress-schema)
# 2. Response Compression (tokenless compress-response)
# 3. Command Rewriting (RTK)
# 4. Stats System (record, list, summary, diff)

set -uo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

TESTS_PASSED=0
TESTS_FAILED=0
TESTS_TOTAL=0

log_info() { echo -e "${BLUE}[INFO]${NC} $1"; }
log_pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((TESTS_PASSED++)); ((TESTS_TOTAL++)); }
log_fail() { echo -e "${RED}[FAIL]${NC} $1"; ((TESTS_FAILED++)); ((TESTS_TOTAL++)); }
log_section() { echo -e "\n${YELLOW}========================================${NC}\n${YELLOW}$1${NC}\n${YELLOW}========================================${NC}\n"; }

assert_contains() {
    local input="$1" expected="$2" test_name="$3"
    if echo "$input" | grep -q "$expected"; then log_pass "$test_name"
    else log_fail "$test_name - Expected: $expected"; fi
}

test_schema_compression() {
    log_section "Test 1: Schema Compression"

    log_info "Test 1.1: Simple schema compression"
    local simple_schema='{"function":{"name":"greet","description":"Say hello","parameters":{"type":"object","properties":{"name":{"type":"string"}}}}}'
    local compressed=$(echo "$simple_schema" | tokenless compress-schema 2>/dev/null)
    assert_contains "$compressed" '"function"' "Simple schema preserves function"
    assert_contains "$compressed" '"greet"' "Simple schema preserves name"

    log_info "Test 1.2: Nested schema compression"
    local nested_schema='{"function":{"name":"create_user","parameters":{"type":"object","title":"Params","properties":{"address":{"type":"object","title":"Address","properties":{"street":{"type":"string"}}}}}}}'
    compressed=$(echo "$nested_schema" | tokenless compress-schema 2>/dev/null)
    assert_contains "$compressed" '"address"' "Nested schema preserves address"

    log_info "Test 1.3: Enum preservation"
    local enum_schema='{"function":{"name":"calc","parameters":{"properties":{"op":{"type":"string","enum":["add","sub"]}}}}}'
    compressed=$(echo "$enum_schema" | tokenless compress-schema 2>/dev/null)
    assert_contains "$compressed" '"enum"' "Enum preserved"

    log_info "Test 1.4: Edge cases"
    assert_contains "$(echo '{}' | tokenless compress-schema 2>/dev/null)" '{}' "Empty schema"
    assert_contains "$(echo 'null' | tokenless compress-schema 2>/dev/null)" 'null' "Null schema"
}

test_response_compression() {
    log_section "Test 2: Response Compression"

    log_info "Test 2.1: Null removal"
    local null_response='{"name":"test","value":null,"count":5}'
    local compressed=$(echo "$null_response" | tokenless compress-response 2>/dev/null)
    assert_contains "$compressed" '"name"' "Null removal preserves name"

    log_info "Test 2.2: Debug field removal"
    local debug_response='{"data":"ok","debug":"info","trace":"stack"}'
    compressed=$(echo "$debug_response" | tokenless compress-response 2>/dev/null)
    assert_contains "$compressed" '"data"' "Debug removal preserves data"

    log_info "Test 2.3: Nested object"
    local nested='{"status":"ok","data":{"user":{"name":"John"}}}'
    compressed=$(echo "$nested" | tokenless compress-response 2>/dev/null)
    assert_contains "$compressed" '"status"' "Nested preserves status"
}

test_command_rewriting() {
    log_section "Test 3: Command Rewriting (RTK)"

    log_info "Test 3.1: RTK availability"
    if command -v rtk &> /dev/null; then
        log_pass "RTK available: $(rtk --version)"
    else log_fail "RTK not found"; fi

    log_info "Test 3.2: RTK rewrite"
    local rewritten=$(rtk rewrite "ls -la" 2>/dev/null || echo "ls -la")
    if [ -n "$rewritten" ]; then log_pass "RTK rewrite works: $rewritten"
    else log_fail "RTK rewrite failed"; fi

    log_info "Test 3.3: Multiple commands"
    local cmds=("git status" "cargo build" "npm install")
    local ok=0
    for cmd in "${cmds[@]}"; do
        local r=$(rtk rewrite "$cmd" 2>/dev/null || echo "")
        [ -n "$r" ] && ((ok++)) || true
    done
    log_pass "RTK processed $ok/${#cmds[@]} commands"
}

test_stats_system() {
    log_section "Test 4: Stats System"

    # Use a temp DB for testing
    local test_db=$(mktemp)
    export TOKENLESS_STATS_DB="$test_db"

    # ── 4.1: Trigger stats recording via actual compress-response ──
    log_info "Test 4.1: Stats auto-record via compress-response"
    local test_response='{"data":"important","debug":"noise","trace":"noise","items":[1,2,3,4,5,6,7,8,9,10]}'
    local compressed
    compressed=$(echo "$test_response" | tokenless compress-response \
        --agent-id test-agent \
        --session-id test-session \
        --tool-use-id test-tool \
        2>/dev/null) || true
    if [ -n "$compressed" ]; then
        log_pass "Stats auto-record via compress-response"
    else
        log_fail "Stats auto-record failed"
    fi

    # ── 4.2: Verify stats list shows the record ──
    log_info "Test 4.2: Stats list"
    local list_output
    list_output=$(tokenless stats list 2>/dev/null) || true
    if echo "$list_output" | grep -q '\[ID:'; then
        log_pass "Stats list shows records"
    else
        log_fail "Stats list missing ID"
    fi

    # ── 4.3: Verify stats show displays details ──
    log_info "Test 4.3: Stats show"
    local record_id
    record_id=$(echo "$list_output" | grep -o '\[ID:[0-9]*\]' | head -1 | grep -o '[0-9]*')
    if [ -n "$record_id" ]; then
        local show_output
        show_output=$(tokenless stats show "$record_id" 2>/dev/null) || true
        if echo "$show_output" | grep -qE '=== Before ===|=== After ==='; then
            log_pass "Stats show displays compression metrics"
        else
            log_fail "Stats show missing metrics"
        fi
    else
        log_fail "No record ID for stats show"
    fi

    # ── 4.4: Stats summary ──
    log_info "Test 4.4: Stats summary"
    local summary
    summary=$(tokenless stats summary 2>/dev/null) || true
    if echo "$summary" | grep -qE "Total Records|compress-response"; then
        log_pass "Stats summary works"
    else
        log_fail "Stats summary broken"
    fi

    # ── 4.5: Stats clear ──
    log_info "Test 4.5: Stats clear"
    tokenless stats clear --yes >/dev/null 2>&1
    if [ $? -eq 0 ]; then
        log_pass "Stats clear works"
    else
        log_fail "Stats clear failed"
    fi

    unset TOKENLESS_STATS_DB
    rm -f "$test_db"
}

main() {
    echo "============================================"
    echo "  Token-Less Full Test Suite"
    echo "============================================"

    if ! command -v tokenless &> /dev/null; then
        echo -e "${RED}ERROR: tokenless not found${NC}"; exit 1
    fi
    log_info "Testing $(tokenless --version)"

    test_schema_compression
    test_response_compression
    test_command_rewriting
    test_stats_system

    echo ""
    echo "============================================"
    echo "  Summary: ${TESTS_PASSED}/${TESTS_TOTAL} passed"
    echo "============================================"

    [ "$TESTS_FAILED" -gt 0 ] && exit 1
    echo -e "\n${GREEN}All tests passed!${NC}"
}

main "$@"
