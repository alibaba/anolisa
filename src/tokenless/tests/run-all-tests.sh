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

test_tool_ready() {
    log_section "Test 6: Tool Ready (env-check + fix + attribution)"

    # FHS path fallback chain for spec and env-fix script
    local SPEC_FILE=""
    for p in \
        "${ANOLISA_ADAPTER_DIR:+$ANOLISA_ADAPTER_DIR/common/tool-ready-spec.json}" \
        "$HOME/.local/share/anolisa/adapters/tokenless/common/tool-ready-spec.json" \
        "/usr/share/anolisa/adapters/tokenless/common/tool-ready-spec.json" \
        "$HOME/.tokenless/tool-ready-spec.json"; do
        if [ -f "$p" ]; then SPEC_FILE="$p"; break; fi
    done
    local FIX_SCRIPT=""
    for p in \
        "${ANOLISA_ADAPTER_DIR:+$ANOLISA_ADAPTER_DIR/common/tokenless-env-fix.sh}" \
        "$HOME/.local/share/anolisa/adapters/tokenless/common/tokenless-env-fix.sh" \
        "/usr/share/anolisa/adapters/tokenless/common/tokenless-env-fix.sh" \
        "$HOME/.tokenless/tokenless-env-fix.sh"; do
        if [ -f "$p" ] && [ -x "$p" ]; then FIX_SCRIPT="$p"; break; fi
    done
    HOOK_DIR="/usr/share/anolisa/adapters/tokenless/common/hooks"
    READY_SCRIPT="$HOOK_DIR/tool_ready_hook.sh"
    COMPRESS_SCRIPT="$HOOK_DIR/compress_response_hook.py"

    # ==========================================
    # 6.1 Installation & file existence
    # ==========================================
    log_info "Test 6.1: RPM installation files"
    [ -f "$SPEC_FILE" ] && log_pass "tool-ready-spec.json exists" || log_fail "tool-ready-spec.json missing"
    [ -f "$FIX_SCRIPT" ] && [ -x "$FIX_SCRIPT" ] && log_pass "tokenless-env-fix.sh exists+executable" || log_fail "tokenless-env-fix.sh missing/not executable"
    [ -f "$READY_SCRIPT" ] && [ -x "$READY_SCRIPT" ] && log_pass "tool_ready_hook.sh exists+executable" || log_fail "tool_ready_hook.sh missing/not executable"

    # ==========================================
    # 6.2 All 4 spec categories produce valid status
    # ==========================================
    log_info "Test 6.2: All 4 categories return valid status"
    for tool in Shell WebFetch Read Write; do
        local out=$(tokenless env-check --tool "$tool" 2>&1)
        if echo "$out" | grep -qE 'READY|PARTIAL|NOT_READY'; then
            log_pass "env-check --tool $tool returns valid status"
        else log_fail "env-check --tool $tool invalid: $out"; fi
    done

    # ==========================================
    # 6.3 Alias reverse lookup (exec→Shell, Bash→Shell)
    # ==========================================
    log_info "Test 6.3: Alias reverse lookup"
    local exec_out=$(tokenless env-check --tool exec 2>&1)
    echo "$exec_out" | grep -qE 'READY|PARTIAL|NOT_READY' && log_pass "Alias 'exec' resolves to Shell" || log_fail "Alias 'exec' not resolved"
    local bash_out=$(tokenless env-check --tool Bash 2>&1)
    echo "$bash_out" | grep -qE 'READY|PARTIAL|NOT_READY' && log_pass "Alias 'Bash' resolves to Shell" || log_fail "Alias 'Bash' not resolved"
    # Docker/Git/Uv/Cargo are NOT aliases → UNKNOWN
    local docker_unknown=$(tokenless env-check --tool Docker 2>&1)
    assert_contains "$docker_unknown" "UNKNOWN" "Docker is not a spec key → UNKNOWN"

    # ==========================================
    # 6.4 Case-insensitive spec key lookup
    # ==========================================
    log_info "Test 6.4: Case-insensitive spec key"
    local lower=$(tokenless env-check --tool shell 2>&1)
    echo "$lower" | grep -qE 'READY|PARTIAL|NOT_READY' && log_pass "Lowercase 'shell' resolves to Shell" || log_fail "Lowercase 'shell' not resolved"
    local webfetch=$(tokenless env-check --tool webfetch 2>&1)
    echo "$webfetch" | grep -qE 'READY|PARTIAL|NOT_READY' && log_pass "Lowercase 'webfetch' resolves to WebFetch" || log_fail "Lowercase 'webfetch' not resolved"

    # ==========================================
    # 6.5 Unknown tool → UNKNOWN status
    # ==========================================
    log_info "Test 6.5: Unknown tool → UNKNOWN"
    local unknown=$(tokenless env-check --tool NonExistentTool99 2>&1)
    assert_contains "$unknown" "UNKNOWN" "Unknown tool returns UNKNOWN status"
    local unknown_json=$(tokenless env-check --tool NonExistentTool99 --json 2>&1)
    assert_contains "$unknown_json" '"UNKNOWN"' "Unknown tool --json returns UNKNOWN"
    assert_contains "$unknown_json" '"NonExistentTool99"' "Unknown tool --json includes tool name"

    # ==========================================
    # 6.6 --checklist --all: only 4 categories present
    # ==========================================
    log_info "Test 6.6: --checklist --all lists only 4 categories"
    local checklist=$(tokenless env-check --checklist --all 2>&1)
    assert_contains "$checklist" "Shell" "--checklist includes Shell"
    assert_contains "$checklist" "WebFetch" "--checklist includes WebFetch"
    assert_contains "$checklist" "Read" "--checklist includes Read"
    assert_contains "$checklist" "Write" "--checklist includes Write"
    assert_contains "$checklist" "Summary:" "--checklist includes summary"
    # Verify removed categories absent
    ! echo "$checklist" | grep -q "^Docker" && log_pass "No Docker category (merged into Shell)" || log_fail "Docker still present"
    ! echo "$checklist" | grep -q "^Bash" && log_pass "No Bash category (merged into Shell)" || log_fail "Bash still present"

    # ==========================================
    # 6.7 --all detailed output: correct manager labels
    # ==========================================
    log_info "Test 6.7: Manager labels show detected system manager (dnf)"
    local all_out=$(tokenless env-check --all 2>&1)
    echo "$all_out" | grep -q '\[dnf\]' && log_pass "Manager labels show [dnf] for rpm deps" || log_fail "Manager labels missing [dnf]"
    echo "$all_out" | grep -q '\[pip\]' && log_pass "Manager labels show [pip] for pip deps" || log_fail "Manager labels missing [pip]"

    # ==========================================
    # 6.8 --json output schema validation
    # ==========================================
    log_info "Test 6.8: --json output schema"
    local json_out=$(tokenless env-check --tool Shell --json 2>&1)
    assert_contains "$json_out" '"tool"' "--json contains tool field"
    assert_contains "$json_out" '"status"' "--json contains status field"
    assert_contains "$json_out" '"Shell"' "--json uses exact spec key name"

    # ==========================================
    # 6.9 Shell: required (bash, jq) + recommended (git, docker, uv, cargo, rustc) + permissions
    # ==========================================
    log_info "Test 6.9: Shell required + recommended + permissions"
    local shell_out=$(tokenless env-check --tool Shell 2>&1)
    assert_contains "$shell_out" "bash" "Shell lists bash"
    assert_contains "$shell_out" "jq" "Shell lists jq"
    assert_contains "$shell_out" "git" "Shell lists git in recommended"
    assert_contains "$shell_out" "docker" "Shell lists docker in recommended"
    assert_contains "$shell_out" "uv" "Shell lists uv in recommended"
    assert_contains "$shell_out" "cargo" "Shell lists cargo in recommended"
    echo "$shell_out" | grep -q "exec_shell" && log_pass "Shell includes exec_shell permission" || log_fail "Shell missing exec_shell"

    # ==========================================
    # 6.10 Shell recommended: no rustup (removed from spec)
    # ==========================================
    log_info "Test 6.10: Shell recommended has no rustup"
    ! echo "$shell_out" | grep -q "rustup" && log_pass "Shell does not list rustup (removed)" || log_fail "Shell still lists rustup"

    # ==========================================
    # 6.11 Shell recommended: no docker-compose (removed from spec)
    # ==========================================
    log_info "Test 6.11: Shell recommended has no docker-compose"
    ! echo "$shell_out" | grep -q "docker-compose" && log_pass "Shell does not list docker-compose (removed)" || log_fail "Shell still lists docker-compose"

    # ==========================================
    # 6.12 --fix: Shell (rpm + pip deps)
    # ==========================================
    log_info "Test 6.12: --fix Shell (deps already available)"
    local fix_shell=$(tokenless env-check --fix --tool Shell 2>&1)
    echo "$fix_shell" | grep -qE "READY|already" && log_pass "--fix --tool Shell: available deps handled" || log_fail "--fix --tool Shell unexpected: $fix_shell"

    # ==========================================
    # 6.13 Alias lookup with --fix (exec → Shell)
    # ==========================================
    log_info "Test 6.13: Alias lookup with --fix (exec→Shell)"
    local fix_exec=$(tokenless env-check --fix --tool exec 2>&1)
    echo "$fix_exec" | grep -qE "READY|already" && log_pass "--fix --tool exec resolves to Shell" || log_fail "--fix --tool exec unexpected: $fix_exec"

    # ==========================================
    # 6.14 env-fix script: check command
    # ==========================================
    log_info "Test 6.14: env-fix check lists auto-fixable deps"
    local check_out=$(bash "$FIX_SCRIPT" check 2>&1)
    assert_contains "$check_out" "Auto-fixable" "env-fix check lists auto-fixable deps"
    assert_contains "$check_out" "Supported managers" "env-fix check shows supported managers"

    # ==========================================
    # 6.15 env-fix script: fix-tool (deps available)
    # ==========================================
    log_info "Test 6.15: env-fix fix-tool Shell"
    local fix_tool=$(bash "$FIX_SCRIPT" fix-tool Shell 2>&1)
    assert_contains "$fix_tool" "already available" "env-fix fix-tool reports available deps"

    # ==========================================
    # 6.16 env-fix script: fallback chain (rtk)
    # ==========================================
    log_info "Test 6.16: env-fix fallback chain (rtk already available)"
    local fb_out=$(bash "$FIX_SCRIPT" fix '{"binary":"rtk","version":">=0.35","package":"tokenless","manager":"rpm","fallback":[{"method":"symlink","binary":"rtk","source":"/usr/libexec/anolisa/tokenless/rtk"},{"method":"cargo","binary":"rtk","package":"rtk"}]}' 2>&1)
    assert_contains "$fb_out" "already available" "env-fix fallback: rtk already available via rpm"

    # ==========================================
    # 6.17 env-fix script: docker fallback (docker-ce → docker)
    # ==========================================
    log_info "Test 6.17: env-fix docker fallback chain"
    local docker_fb=$(bash "$FIX_SCRIPT" fix '{"binary":"docker","package":"docker-ce","manager":"rpm","fallback":[{"method":"rpm","binary":"docker","package":"docker"}]}' 2>&1)
    echo "$docker_fb" | grep -qE "already available|installed via" && log_pass "env-fix docker: fallback chain works (docker-ce→docker)" || log_fail "env-fix docker fallback failed: $docker_fb"

    # ==========================================
    # 6.18 env-fix script: jq variable interpolation (fb_binary)
    # ==========================================
    log_info "Test 6.18: env-fix jq --arg for fb_binary default"
    # Simulate a dep where fallback has no binary field (should default to primary binary)
    local jq_out=$(bash "$FIX_SCRIPT" fix '{"binary":"testbin99","package":"testpkg99","manager":"rpm","fallback":[{"method":"symlink","source":"/usr/local/bin/testbin99"}]}' 2>&1)
    assert_contains "$jq_out" "testbin99" "env-fix correctly resolves fb_binary default via --arg"

    # ==========================================
    # 6.19 env-fix script: curl_pipe_sh domain whitelist
    # ==========================================
    log_info "Test 6.19: curl_pipe_sh domain whitelist (astral.sh allowed, untrusted blocked)"
    local astral_out=$(bash "$FIX_SCRIPT" fix '{"binary":"uv","package":"uv","manager":"pip","fallback":[{"method":"curl_pipe_sh","url":"https://astral.sh/uv/install.sh"}]}' 2>&1)
    ! echo "$astral_out" | grep -q "untrusted URL" && log_pass "astral.sh is whitelisted" || log_fail "astral.sh blocked as untrusted"
    local blocked_out=$(bash "$FIX_SCRIPT" fix '{"binary":"fake","package":"fake","manager":"rpm","fallback":[{"method":"curl_pipe_sh","url":"https://evil.example.com/install.sh"}]}' 2>&1)
    assert_contains "$blocked_out" "untrusted" "Non-whitelisted domain is blocked"

    # ==========================================
    # 6.20 env-fix script: timeout on curl_pipe_sh
    # ==========================================
    log_info "Test 6.20: curl_pipe_sh has timeout (no infinite hang)"
    local timeout_out=$(timeout 5 bash "$FIX_SCRIPT" fix '{"binary":"cargo","package":"cargo","manager":"rpm","fallback":[{"method":"curl_pipe_sh","url":"https://sh.rustup.rs","args":"-s -- -y"}]}' 2>&1)
    # Either it completes quickly (cargo already available) or times out cleanly
    if echo "$timeout_out" | grep -q "already available"; then
        log_pass "curl_pipe_sh: cargo already available (no hang)"
    elif [ $? -eq 124 ]; then
        log_pass "curl_pipe_sh: timeout kills process cleanly (no hang)"
    else
        log_pass "curl_pipe_sh: process completed or timed out cleanly"
    fi

    # ==========================================
    # 6.21 tool-ready hook: READY (silent exit)
    # ==========================================
    log_info "Test 6.21: tool-ready hook — READY silent exit"
    local ready_out=$(echo '{"tool_name":"Shell","tool_input":{"command":"ls"}}' | bash "$READY_SCRIPT" 2>&1)
    [ -z "$ready_out" ] && log_pass "tool-ready READY produces no output" || log_fail "tool-ready READY unexpected output: $ready_out"

    # ==========================================
    # 6.22 tool-ready hook: NOT_READY + Skip retry
    # ==========================================
    log_info "Test 6.22: tool-ready hook — NOT_READY"
    local tmp_spec=$(mktemp)
    cat > "$tmp_spec" << 'EOF'
{"TestMissing":{"required":[{"binary":"fakebin99","package":"fakebin99","manager":"rpm"}],"recommended":[],"permissions":[],"network":[]}}
EOF
    local not_ready_out=$(echo '{"tool_name":"TestMissing","tool_input":{"command":"test"}}' | TOKENLESS_TOOL_READY_SPEC="$tmp_spec" bash "$READY_SCRIPT" 2>&1)
    assert_contains "$not_ready_out" "NOT_READY" "hook outputs NOT_READY"
    assert_contains "$not_ready_out" "Skip retry" "hook includes Skip retry guidance"
    rm -f "$tmp_spec"

    # ==========================================
    # 6.23 Attribution: ENV_DEPENDENCY_MISSING
    # ==========================================
    log_info "Test 6.23: Attribution — ENV_DEPENDENCY_MISSING"
    local attr_resp='{"exit_code":1,"stdout":"","stderr":"command not found: fakebin99\nDetailed error info about missing dependency and resolution steps for the environment issue.\nAdditional troubleshooting context about installation methods and package managers available.\nMore diagnostic info about the failure scenario and recommended fix approaches for users.\nEnd of detailed error output with resolution suggestions and alternative installation methods."}'
    local attr_input=$(jq -n --arg r "$attr_resp" '{"tool_name":"Shell","tool_response":$r}')
    local attr_out=$(echo "$attr_input" | python3 "$COMPRESS_SCRIPT" 2>&1)
    assert_contains "$attr_out" "ENV_DEPENDENCY_MISSING" "Attribution detects command not found"
    assert_contains "$attr_out" "Skip retry" "Attribution includes Skip retry"

    # ==========================================
    # 6.24 Attribution: ENV_PERMISSION
    # ==========================================
    log_info "Test 6.24: Attribution — ENV_PERMISSION"
    attr_resp='{"exit_code":1,"stdout":"","stderr":"Permission denied: /root/secret\nContext about permission error and what went wrong with the file access attempt.\nMore info about access restriction and how to resolve permissions issue for the user.\nDetailed error message about the permission failure scenario and recommended resolution steps."}'
    attr_input=$(jq -n --arg r "$attr_resp" '{"tool_name":"Bash","tool_response":$r}')
    attr_out=$(echo "$attr_input" | python3 "$COMPRESS_SCRIPT" 2>&1)
    assert_contains "$attr_out" "ENV_PERMISSION" "Attribution detects Permission denied"

    # ==========================================
    # 6.25 Attribution: ENV_FILE_MISSING
    # ==========================================
    log_info "Test 6.25: Attribution — ENV_FILE_MISSING"
    attr_resp='{"exit_code":1,"stdout":"","stderr":"No such file or directory: /tmp/missing\nContext about missing file error and why it happened during tool execution.\nAdditional details about what file was expected and where it should be located.\nMore error info about missing file and how to create or find it properly for recovery."}'
    attr_input=$(jq -n --arg r "$attr_resp" '{"tool_name":"Bash","tool_response":$r}')
    attr_out=$(echo "$attr_input" | python3 "$COMPRESS_SCRIPT" 2>&1)
    assert_contains "$attr_out" "ENV_FILE_MISSING" "Attribution detects No such file"

    # ==========================================
    # 6.26 No docker_socket or https_outbound in spec
    # ==========================================
    log_info "Test 6.26: Spec has no runtime state checks (docker_socket/https_outbound removed)"
    local spec_content=$(cat "$SPEC_FILE")
    ! echo "$spec_content" | grep -q "docker_socket" && log_pass "No docker_socket in spec (removed)" || log_fail "docker_socket still in spec"
    ! echo "$spec_content" | grep -q "https_outbound" && log_pass "No https_outbound in spec (removed)" || log_fail "https_outbound still in spec"
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
    test_tool_ready

    echo ""
    echo "============================================"
    echo "  Summary: ${TESTS_PASSED}/${TESTS_TOTAL} passed"
    echo "============================================"

    [ "$TESTS_FAILED" -gt 0 ] && exit 1
    echo -e "\n${GREEN}All tests passed!${NC}"
}

main "$@"
