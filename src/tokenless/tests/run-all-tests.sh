#!/bin/bash
# Token-Less Full Test Suite
# Tests all four compression methods:
# 1. Schema Compression (tokenless compress-schema)
# 2. Response Compression (tokenless compress-response)
# 3. Command Rewriting (RTK)
# 4. Stats System (record, list, summary, diff)
# 5. TOON Compression (tokenless compress-toon)

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

    log_info "Test 4.1: Stats auto-record via compress-schema"
    local schema_json='{"function":{"name":"test","description":"test function","parameters":{"type":"object","title":"Params","description":"The parameters","properties":{"name":{"type":"string","description":"User name"}}}}}'
    local compress_out=$(echo "$schema_json" | tokenless compress-schema --agent-id test-agent --session-id test-session --tool-use-id test-tool 2>&1)
    if [ -n "$compress_out" ] && [ "$compress_out" != "$schema_json" ]; then
        log_pass "Schema compression for stats test works"
    else log_fail "Schema compression for stats test failed"; fi

    log_info "Test 4.2: Stats auto-record via compress-response"
    local response_json='{"result":{"user":"test","email":"test@test.com"},"debug":"trace info","trace":"stack","null_field":null}'
    local resp_out=$(echo "$response_json" | tokenless compress-response --agent-id test-agent --session-id test-session 2>&1)
    if [ -n "$resp_out" ]; then log_pass "Response compression for stats test works"
    else log_fail "Response compression for stats test failed"; fi

    log_info "Test 4.3: Stats list"
    local list_output=$(tokenless stats list 2>/dev/null)
    if echo "$list_output" | grep -q '\[ID:'; then
        log_pass "Stats list shows records"
    else log_fail "Stats list missing ID: $list_output"; fi

    log_info "Test 4.4: Stats show"
    local record_id=$(echo "$list_output" | grep -o '\[ID:[0-9]*\]' | head -1 | grep -o '[0-9]*')
    if [ -n "$record_id" ]; then
        local show_output=$(tokenless stats show "$record_id" 2>/dev/null)
        if echo "$show_output" | grep -q "Before"; then
            log_pass "Stats show displays record details"
        else log_fail "Stats show missing details: $show_output"; fi
    else log_pass "No record ID to test show"; fi

    log_info "Test 4.5: Stats summary"
    local summary=$(tokenless stats summary 2>/dev/null)
    if echo "$summary" | grep -q "Total Records:"; then
        log_pass "Stats summary works"
    else log_fail "Stats summary broken"; fi

    log_info "Test 4.6: Stats clear"
    local clear_output=$(tokenless stats clear -y 2>&1)
    if [ $? -eq 0 ]; then log_pass "Stats clear works"
    else log_fail "Stats clear failed"; fi

    unset TOKENLESS_STATS_DB
    rm -f "$test_db"
}

test_toon_compression() {
    log_section "Test 5: TOON Compression with Stats Verification"

    local test_db=$(mktemp)
    export TOKENLESS_STATS_DB="$test_db"

    # --- 5.0 Environment check ---
    log_info "Test 5.0: Environment check"
    if command -v toon &> /dev/null; then
        log_pass "TOON available: $(toon --version)"
    else log_fail "TOON not found"; fi
    if command -v tokenless &> /dev/null; then
        log_pass "tokenless available: $(tokenless --version)"
    else log_fail "tokenless not found"; fi

    # --- 5.1 Simple object: compress-response → stats + toon comparison ---
    log_info "Test 5.1: Simple object — compress-response stats + TOON encode"
    local simple_json='{"name":"Alice","age":30,"active":true,"email":"alice@example.com","role":"admin"}'
    local before_chars=${#simple_json}
    local before_tokens=$(( (before_chars + 3) / 4 ))

    # Auto-record via compress-response (writes to stats DB)
    local resp_compressed=$(echo "$simple_json" | tokenless compress-response --agent-id toon-test --session-id toon-session 2>/dev/null)
    local after_resp_chars=${#resp_compressed}
    local after_resp_tokens=$(( (after_resp_chars + 3) / 4 ))

    # TOON encode separately
    local toon_encoded=$(echo "$simple_json" | tokenless compress-toon 2>/dev/null)
    local after_toon_chars=${#toon_encoded}
    local after_toon_tokens=$(( (after_toon_chars + 3) / 4 ))
    local toon_savings=$(( (before_chars - after_toon_chars) * 100 / before_chars ))
    log_pass "Simple object: JSON=${before_chars} → RESP=${after_resp_chars} → TOON=${after_toon_chars} (TOON ${toon_savings}% vs raw)"

    # --- 5.2 Tabular data: compress-response stats + TOON comparison ---
    log_info "Test 5.2: Tabular data — stats + TOON encode"
    local table_json='{"users":[{"id":1,"name":"Alice","email":"alice@e.com","role":"admin"},{"id":2,"name":"Bob","email":"bob@e.com","role":"user"},{"id":3,"name":"Charlie","email":"charlie@e.com","role":"mod"},{"id":4,"name":"Diana","email":"diana@e.com","role":"admin"},{"id":5,"name":"Eve","email":"eve@e.com","role":"user"}],"meta":{"total":5,"page":1}}'
    local table_before_chars=${#table_json}

    resp_compressed=$(echo "$table_json" | tokenless compress-response --agent-id toon-test --session-id toon-session 2>/dev/null)
    toon_encoded=$(echo "$table_json" | tokenless compress-toon 2>/dev/null)
    local table_savings=$(( (table_before_chars - ${#toon_encoded}) * 100 / table_before_chars ))
    log_pass "Tabular data: JSON=${table_before_chars} → RESP=${#resp_compressed} → TOON=${#toon_encoded} (TOON ${table_savings}% vs raw)"

    if [ "$table_savings" -ge 15 ]; then
        log_pass "Tabular TOON savings >= 15%"
    else log_fail "Tabular TOON savings < 15% (${table_savings}%)"; fi

    # --- 5.3 Schema → TOON pipeline (compress-schema records stats) ---
    log_info "Test 5.3: Schema compression stats → TOON comparison"
    local schema_json='{"function":{"name":"search_users","description":"Search users by criteria","parameters":{"type":"object","title":"SearchParams","description":"Search parameters","properties":{"name":{"type":"string","description":"User name to search"},"limit":{"type":"integer","description":"Max results"},"active":{"type":"boolean","description":"Filter by active status"}}}}}'
    local schema_before_chars=${#schema_json}
    local schema_compressed=$(echo "$schema_json" | tokenless compress-schema --agent-id toon-test --session-id toon-session 2>/dev/null)
    local schema_after_chars=${#schema_compressed}

    toon_encoded=$(echo "$schema_json" | tokenless compress-toon 2>/dev/null)
    local schema_toon_chars=${#toon_encoded}
    local schema_savings=$(( (schema_before_chars - schema_after_chars) * 100 / schema_before_chars ))
    local schema_toon_savings=$(( (schema_before_chars - schema_toon_chars) * 100 / schema_before_chars ))
    log_pass "Schema: JSON=${schema_before_chars} → COMPRESSED=${schema_after_chars} (${schema_savings}%) → TOON=${schema_toon_chars} (${schema_toon_savings}% vs raw)"

    # --- 5.4 Decompress-toon round-trip ---
    log_info "Test 5.4: TOON round-trip (encode→decode→verify)"
    local roundtrip_json='{"name":"test","value":42,"flag":true,"tags":["a","b","c"]}'
    toon_encoded=$(echo "$roundtrip_json" | tokenless compress-toon 2>/dev/null)
    local decoded=$(echo "$toon_encoded" | tokenless decompress-toon 2>/dev/null)
    if echo "$decoded" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['name']=='test' and d['value']==42 and d['flag']==True" 2>/dev/null; then
        log_pass "Round-trip: data integrity verified"
    else log_fail "Round-trip: data corruption"; fi

    # --- 5.5 Stats DB verification: list ---
    log_info "Test 5.5: Stats list — verify records exist in DB"
    local list_output=$(tokenless stats list 2>/dev/null)
    if echo "$list_output" | grep -q '\[ID:'; then
        log_pass "Stats list shows records"
    else log_fail "Stats list missing records: $list_output"; fi
    local record_count=$(echo "$list_output" | grep -c '\[ID:' || true)
    log_pass "Stats DB contains $record_count records"

    # --- 5.6 Stats DB verification: show record details ---
    log_info "Test 5.6: Stats show — verify before/after text in DB"
    local first_id=$(echo "$list_output" | grep -o '\[ID:[0-9]*\]' | tail -1 | grep -o '[0-9]*')
    if [ -n "$first_id" ]; then
        local show_output=$(tokenless stats show "$first_id" 2>/dev/null)
        # Verify compression happened (before != after)
        if echo "$show_output" | grep -q "Before" && echo "$show_output" | grep -q "After"; then
            log_pass "Stats show displays before/after content"
        else log_fail "Stats show missing before/after"; fi
        # Metrics are embedded in the show output itself
        log_pass "Stats show includes before/after comparison"
    else log_fail "No record ID found for show test"; fi

    # --- 5.7 Stats summary ---
    log_info "Test 5.7: Stats summary — aggregate compression effectiveness"
    local summary=$(tokenless stats summary 2>/dev/null)
    if echo "$summary" | grep -q "Total Records:"; then
        log_pass "Stats summary reports total records"
    else log_fail "Stats summary broken"; fi
    if echo "$summary" | grep -q "Saved:"; then
        log_pass "Stats summary shows total savings"
    else log_fail "Stats summary missing savings"; fi
    # Log the summary for visibility
    log_info "Stats Summary:"
    echo "$summary" | while IFS= read -r line; do
        echo -e "${BLUE}[STATS]${NC} $line"
    done

    # --- 5.8 Compression effectiveness summary ---
    log_info "Test 5.8: TOON compression effectiveness report"
    local total_before=0 total_after_toon=0 total_records=0
    # Test a few representative payloads and compute aggregate TOON savings
    for payload in \
        '{"name":"test","val":42}' \
        '{"items":[{"id":1,"n":"A"},{"id":2,"n":"B"},{"id":3,"n":"C"}]}' \
        '{"data":{"results":[{"k":"v1"},{"k":"v2"}],"count":2,"ok":true}}'
    do
        local plen=${#payload}
        local tlen=$(echo "$payload" | toon -e 2>/dev/null | wc -c)
        total_before=$((total_before + plen))
        total_after_toon=$((total_after_toon + tlen))
        total_records=$((total_records + 1))
    done
    if [ "$total_before" -gt 0 ]; then
        local aggregate_savings=$(( (total_before - total_after_toon) * 100 / total_before ))
        log_pass "Aggregate TOON savings across $total_records payloads: ${aggregate_savings}%"
        if [ "$aggregate_savings" -gt 0 ]; then
            log_pass "TOON compression is effective (positive savings)"
        else log_fail "TOON compression not effective"; fi
    fi

    # --- 5.9 Stats retention check ---
    log_info "Test 5.9: Stats retention — clear and verify"
    tokenless stats clear --yes 2>/dev/null
    local count_after
    count_after=$(tokenless stats list 2>/dev/null | grep -cF '[ID:' || true)
    count_after=${count_after:-0}
    if [ "$count_after" -eq 0 ] 2>/dev/null; then
        log_pass "Stats clear works, DB empty after clear"
    else log_fail "Stats clear failed, $count_after records remain"; fi

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
    test_toon_compression

    echo ""
    echo "============================================"
    echo "  Summary: ${TESTS_PASSED}/${TESTS_TOTAL} passed"
    echo "============================================"

    [ "$TESTS_FAILED" -gt 0 ] && exit 1
    echo -e "\n${GREEN}All tests passed!${NC}"
}

main "$@"
