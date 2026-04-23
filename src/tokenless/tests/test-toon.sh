#!/usr/bin/env bash
# Toon 工具完整测试用例
# 测试编码、解码、往返转换、CLI 选项、OpenClaw 插件集成

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

assert_eq() {
    local expected="$1" actual="$2" test_name="$3"
    if [ "$expected" = "$actual" ]; then pass "$test_name"
    else fail "$test_name - expected: '$expected', got: '$actual'"; fi
}

assert_contains() {
    local input="$1" expected="$2" test_name="$3"
    if echo "$input" | grep -qF "$expected"; then pass "$test_name"
    else fail "$test_name - expected to contain: '$expected'"; fi
}

# ===== 1. 基础编码测试 =====
section "Test 1: 基础编码"

info "1.1: 简单对象编码"
result=$(echo '{"name":"Alice","age":30}' | toon -e)
assert_contains "$result" "name: Alice" "简单对象 - name 字段"
assert_contains "$result" "age: 30" "简单对象 - age 字段"

info "1.2: 布尔值编码"
result=$(echo '{"active":true,"deleted":false}' | toon -e)
assert_contains "$result" "active: true" "布尔值 true"
assert_contains "$result" "deleted: false" "布尔值 false"

info "1.3: Null 值编码"
result=$(echo '{"value":null}' | toon -e)
assert_contains "$result" "value: null" "Null 值"

info "1.4: 数字编码"
result=$(echo '{"int":42,"float":3.14,"neg":-5}' | toon -e)
assert_contains "$result" "int: 42" "整数"
assert_contains "$result" "float: 3.14" "浮点数"
assert_contains "$result" "neg: -5" "负数"

# ===== 2. 数组编码测试 =====
section "Test 2: 数组编码"

info "2.1: 原始值数组"
result=$(echo '{"tags":["reading","gaming","coding"]}' | toon -e)
assert_eq "tags[3]: reading,gaming,coding" "$result" "原始值数组"

info "2.2: 表格数组（同构对象）"
result=$(echo '{"users":[{"id":1,"name":"Alice"},{"id":2,"name":"Bob"}]}' | toon -e)
assert_contains "$result" "users[2]" "表格数组头部"
assert_contains "$result" "1,Alice" "表格数组 - 第一行"
assert_contains "$result" "2,Bob" "表格数组 - 第二行"

info "2.3: 嵌套数组"
result=$(echo '{"items":[[1,2],[3,4]]}' | toon -e)
assert_contains "$result" "items[2]" "嵌套数组头部"

info "2.4: 空数组"
result=$(echo '{"items":[]}' | toon -e)
assert_eq "items[0]:" "$result" "空数组"

# ===== 3. 解码测试 =====
section "Test 3: 解码"

info "3.1: 简单对象解码"
result=$(echo -e "name: Alice\nage: 30" | toon -d)
# JSON输出紧凑格式，无空格
assert_contains "$result" '"name":"Alice"' "解码 - name"
assert_contains "$result" '"age":30' "解码 - age"

info "3.2: 表格数组解码"
echo -e "users[2]{id,name}:\n  1,Alice\n  2,Bob" | toon -d > /tmp/toon_decode_test.json
result=$(cat /tmp/toon_decode_test.json)
assert_contains "$result" '"users"' "解码表格数组 - users 键"
# id 是数字，name 是字符串
if echo "$result" | grep -q '"id":1'; then pass "解码表格数组 - id 为数字"
else fail "解码表格数组 - id 格式"; fi

# ===== 4. 往返转换测试 =====
section "Test 4: 往返转换"

info "4.1: 简单对象往返"
original='{"name":"Alice","age":30,"active":true}'
toon_out=$(echo "$original" | toon -e)
roundtrip=$(echo "$toon_out" | toon -d)
orig_parsed=$(echo "$original" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin),sort_keys=True))" 2>/dev/null)
rt_parsed=$(echo "$roundtrip" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin),sort_keys=True))" 2>/dev/null)
assert_eq "$orig_parsed" "$rt_parsed" "简单对象往返转换"

info "4.2: 嵌套对象往返"
original='{"user":{"profile":{"name":"Alice","age":30}}}'
toon_out=$(echo "$original" | toon -e)
roundtrip=$(echo "$toon_out" | toon -d)
orig_parsed=$(echo "$original" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin),sort_keys=True))")
rt_parsed=$(echo "$roundtrip" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin),sort_keys=True))")
assert_eq "$orig_parsed" "$rt_parsed" "嵌套对象往返转换"

info "4.3: 表格数组往返"
original='{"users":[{"id":1,"name":"Alice"},{"id":2,"name":"Bob"}]}'
toon_out=$(echo "$original" | toon -e)
roundtrip=$(echo "$toon_out" | toon -d)
orig_parsed=$(echo "$original" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin),sort_keys=True))")
rt_parsed=$(echo "$roundtrip" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin),sort_keys=True))")
assert_eq "$orig_parsed" "$rt_parsed" "表格数组往返转换"

info "4.4: 混合类型往返"
original='{"data":{"items":[{"id":1,"name":"test","tags":["a","b"]}],"count":1,"active":true,"meta":null}}'
toon_out=$(echo "$original" | toon -e)
roundtrip=$(echo "$toon_out" | toon -d)
orig_parsed=$(echo "$original" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin),sort_keys=True))")
rt_parsed=$(echo "$roundtrip" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin),sort_keys=True))")
assert_eq "$orig_parsed" "$rt_parsed" "混合类型往返转换"

# ===== 5. CLI 选项测试 =====
section "Test 5: CLI 选项"

info "5.1: 自定义分隔符 - pipe"
result=$(echo '{"items":["a","b","c"]}' | toon -e --delimiter pipe)
assert_contains "$result" "|" "Pipe 分隔符"

info "5.2: 自定义分隔符 - tab"
result=$(echo '{"items":["a","b","c"]}' | toon -e --delimiter tab)
assert_contains "$result" "	" "Tab 分隔符"

info "5.3: 文件输入"
echo '{"from":"file"}' > /tmp/toon_input.json
result=$(toon /tmp/toon_input.json)
assert_contains "$result" "from: file" "文件输入编码"

info "5.4: 文件输出"
toon -e -o /tmp/toon_output.toon /tmp/toon_input.json
result=$(cat /tmp/toon_output.toon)
assert_contains "$result" "from: file" "文件输出编码"

info "5.5: 统计信息"
result=$(echo '{"data":{"meta":{"items":["x","y","z"]}}}' | toon -e --stats 2>&1)
assert_contains "$result" "Tokens" "统计 - Tokens"
assert_contains "$result" "Savings" "统计 - Savings"

info "5.6: Key folding"
result=$(echo '{"data":{"meta":{"items":["x","y"]}}}' | toon -e --fold-keys)
assert_contains "$result" "data.meta.items" "Key folding"

info "5.7: Path expansion"
result=$(echo -e "a.b.c: 1\na.b.d: 2" | toon -d --expand-paths)
assert_contains "$result" '"a"' "Path expansion - 根键"
assert_contains "$result" '"c"' "Path expansion - 值 c"
assert_contains "$result" '"d"' "Path expansion - 值 d"

info "5.8: JSON 缩进输出"
result=$(echo -e "name: test\nvalue: 42" | toon -d --json-indent 4)
assert_contains "$result" "    " "JSON 缩进输出"

# ===== 6. Tokenless CLI 集成测试 =====
section "Test 6: Tokenless CLI 集成 (compress-toon/decompress-toon)"

info "6.1: tokenless compress-toon"
result=$(echo '{"name":"test","value":42}' | tokenless compress-toon 2>/dev/null)
if [ -n "$result" ]; then pass "tokenless compress-toon 输出非空"
else fail "tokenless compress-toon 返回空 (可能需要新版本 tokenless)"; fi

info "6.2: tokenless decompress-toon"
if [ -n "$result" ]; then
    roundtrip=$(echo "$result" | tokenless decompress-toon 2>/dev/null)
    if [ -n "$roundtrip" ]; then pass "tokenless decompress-toon 输出非空"
    else fail "tokenless decompress-toon 返回空"; fi
else fail "跳过 6.2 (6.1 无输出)"; fi

info "6.3: tokenless compress-toon with tabular data"
result=$(echo '{"users":[{"id":1,"name":"Alice"},{"id":2,"name":"Bob"}]}' | tokenless compress-toon 2>/dev/null)
if [ -n "$result" ]; then pass "tokenless compress-toon 表格数据"
else fail "tokenless compress-toon 表格数据返回空"; fi

info "6.4: tokenless CLI toon 子命令可用"
if tokenless --help 2>&1 | grep -q "compress-toon"; then pass "tokenless compress-toon 子命令可用"
else info "  (需要安装新版本 tokenless)"; fi

# ===== 7. 压缩率测试 =====
section "Test 7: 压缩率测试"

info "7.1: 简单对象压缩率"
json_input='{"name":"Alice","age":30,"email":"alice@example.com","active":true,"role":"admin"}'
json_len=${#json_input}
toon_output=$(echo "$json_input" | toon -e)
toon_len=${#toon_output}
savings=$(( (json_len - toon_len) * 100 / json_len ))
info "  JSON: $json_len chars -> TOON: $toon_len chars (${savings}% savings)"
if [ "$toon_len" -lt "$json_len" ]; then pass "简单对象压缩有效"
else fail "简单对象未压缩"; fi

info "7.2: 表格数据压缩率"
json_input='{"users":[{"id":1,"name":"Alice","role":"admin"},{"id":2,"name":"Bob","role":"user"},{"id":3,"name":"Charlie","role":"moderator"}]}'
json_len=${#json_input}
toon_output=$(echo "$json_input" | toon -e)
toon_len=${#toon_output}
savings=$(( (json_len - toon_len) * 100 / json_len ))
info "  JSON: $json_len chars -> TOON: $toon_len chars (${savings}% savings)"
if [ "$savings" -ge 10 ]; then pass "表格数据压缩率 >= 10%"
else fail "表格数据压缩率 < 10%"; fi

info "7.3: 深度嵌套数据压缩率"
json_input='{"data":{"users":[{"id":1,"name":"Alice"},{"id":2,"name":"Bob"}],"meta":{"total":2,"page":1}}}'
json_len=${#json_input}
toon_output=$(echo "$json_input" | toon -e)
toon_len=${#toon_output}
savings=$(( (json_len - toon_len) * 100 / json_len ))
info "  JSON: $json_len chars -> TOON: $toon_len chars (${savings}% savings)"
if [ "$toon_len" -lt "$json_len" ]; then pass "嵌套数据压缩有效"
else fail "嵌套数据未压缩"; fi

# ===== 8. OpenClaw 插件适配测试 =====
section "Test 8: OpenClaw 插件适配"

info "8.1: 插件文件存在"
if [ -f ~/.openclaw/extensions/tokenless-openclaw/index.js ]; then pass "插件 JS 文件存在"
else fail "插件 JS 文件不存在"; fi

info "8.2: 插件包含 toon 检测逻辑"
if grep -q "checkToon" ~/.openclaw/extensions/tokenless-openclaw/index.js; then pass "插件包含 toon 检测"
else fail "插件缺少 toon 检测"; fi

info "8.3: 插件包含 toon 压缩函数"
if grep -q 'execFileSync.*toon' ~/.openclaw/extensions/tokenless-openclaw/index.js; then pass "插件包含 toon 压缩函数"
else fail "插件缺少 toon 压缩函数"; fi

info "8.4: 插件配置文件存在"
if [ -f ~/.openclaw/extensions/tokenless-openclaw/openclaw.plugin.json ]; then pass "插件配置文件存在"
else fail "插件配置文件不存在"; fi

info "8.5: 插件配置包含 toon_compression_enabled"
if grep -q "toon_compression_enabled" ~/.openclaw/extensions/tokenless-openclaw/openclaw.plugin.json; then pass "插件配置包含 toon 选项"
else fail "插件配置缺少 toon 选项"; fi

info "8.6: 插件已启用"
if python3 -c "
import json
with open('$HOME/.openclaw/openclaw.json') as f:
    cfg = json.load(f)
entries = cfg.get('plugins',{}).get('entries',{})
plugin = entries.get('tokenless-openclaw',{})
assert plugin.get('enabled') == True, 'not enabled'
config = plugin.get('config',{})
assert config.get('toon_compression_enabled') == True, 'toon not enabled'
" 2>/dev/null; then pass "插件已启用且 toon 已配置"
else fail "插件未正确配置"; fi

info "8.7: 插件 toon 调用测试（模拟）"
test_json='{"test":"data","value":42}'
toon_result=$(echo "$test_json" | toon -e 2>/dev/null)
if [ -n "$toon_result" ]; then
    before_chars=${#test_json}
    after_chars=${#toon_result}
    savings=$(( (before_chars - after_chars) * 100 / before_chars ))
    pass "Toon 压缩模拟: $before_chars -> $after_chars chars (${savings}% savings)"
else fail "Toon 压缩模拟失败"; fi

# ===== 汇总 =====
echo ""
echo "============================================"
echo "  测试汇总: ${PASS}/${TOTAL} 通过, ${FAIL} 失败"
echo "============================================"

[ "$FAIL" -gt 0 ] && exit 1
echo -e "\n${GREEN}所有测试通过！${NC}"
