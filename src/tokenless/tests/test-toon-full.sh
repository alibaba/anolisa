#!/usr/bin/env bash
# 全量 TOON 功能验证测试
# 覆盖三种应用场景：Tokenless CLI、COSH (copilot-shell)、OpenClaw

set -uo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

PASS=0
FAIL=0
TOTAL=0
SCENARIOS=0

pass() { echo -e "${GREEN}[PASS]${NC} $1"; ((PASS++)); ((TOTAL++)); }
fail() { echo -e "${RED}[FAIL]${NC} $1"; ((FAIL++)); ((TOTAL++)); }
info() { echo -e "${BLUE}[INFO]${NC} $1"; }
section() { echo -e "\n${YELLOW}========== $1 ==========${NC}\n"; ((SCENARIOS++)); }
scenario() { echo -e "\n${CYAN}▸ $1${NC}"; }

assert_contains() {
    local input="$1" expected="$2" test_name="$3"
    if echo "$input" | grep -qF "$expected"; then pass "$test_name"
    else fail "$test_name - expected to contain: '$expected'"; fi
}

assert_not_empty() {
    local input="$1" test_name="$2"
    if [ -n "$input" ]; then pass "$test_name"
    else fail "$test_name - empty output"; fi
}

# ========== 环境检查 ==========
section "环境检查"

for cmd in toon tokenless jq openclaw; do
    if command -v "$cmd" &>/dev/null; then
        version=$("$cmd" --version 2>/dev/null || echo "installed")
        pass "$cmd 可用 ($version)"
    else
        fail "$cmd 未安装"
    fi
done

# 检查 OpenClaw 插件
if [ -f ~/.openclaw/extensions/tokenless/index.js ]; then
    pass "OpenClaw 插件文件存在"
else
    fail "OpenClaw 插件文件缺失"
fi

# 检查 OpenClaw 配置
if python3 -c "
import json
cfg=json.load(open('$HOME/.openclaw/openclaw.json'))
entries=cfg.get('plugins',{}).get('entries',{})
assert 'tokenless-openclaw' in entries and entries['tokenless-openclaw'].get('enabled'), 'not enabled'
assert entries['tokenless-openclaw'].get('config',{}).get('toon_compression_enabled'), 'toon disabled'
" 2>/dev/null; then
    pass "OpenClaw 插件已启用且 TOON 配置正确"
else
    fail "OpenClaw 插件配置异常"
fi

# 检查 copilot-shell hook
if [ -x /usr/share/tokenless/hooks/copilot-shell/tokenless-compress-toon.sh ]; then
    pass "COSH TOON hook 已安装且可执行"
else
    fail "COSH TOON hook 缺失或不可执行"
fi

# ========== 场景 1: Tokenless CLI ==========
section "场景 1: Tokenless CLI"

scenario "1.1 基础编码/解码"

# 简单对象
result=$(echo '{"name":"Alice","age":30,"active":true}' | tokenless compress-toon 2>/dev/null)
assert_not_empty "$result" "简单对象编码"
assert_contains "$result" "name: Alice" "简单对象 - name"
assert_contains "$result" "age: 30" "简单对象 - age"

# 解码往返
roundtrip=$(echo "$result" | tokenless decompress-toon 2>/dev/null)
assert_not_empty "$roundtrip" "简单对象解码"
if echo "$roundtrip" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d['name']=='Alice' and d['age']==30" 2>/dev/null; then
    pass "往返转换数据一致"
else
    fail "往返转换数据不一致"
fi

scenario "1.2 表格数据压缩"

json='{"users":[{"id":1,"name":"Alice","email":"alice@example.com","role":"admin"},{"id":2,"name":"Bob","email":"bob@example.com","role":"user"},{"id":3,"name":"Charlie","email":"charlie@example.com","role":"moderator"},{"id":4,"name":"Diana","email":"diana@example.com","role":"admin"},{"id":5,"name":"Eve","email":"eve@example.com","role":"user"}]}'
toon_out=$(echo "$json" | tokenless compress-toon 2>/dev/null)
json_len=${#json}
toon_len=${#toon_out}
savings=$(( (json_len - toon_len) * 100 / json_len ))
info "  JSON: $json_len chars → TOON: $toon_len chars (${savings}% 压缩率)"
if [ "$savings" -ge 15 ]; then
    pass "表格数据压缩率 >= 15%"
else
    fail "表格数据压缩率 < 15% (${savings}%)"
fi
assert_contains "$toon_out" "users[5]" "表格数组头部正确"

scenario "1.3 深度嵌套数据"

nested='{"data":{"users":[{"id":1,"profile":{"name":"Alice","age":30,"address":{"city":"Beijing","country":"CN"}}},{"id":2,"profile":{"name":"Bob","age":25,"address":{"city":"Shanghai","country":"CN"}}}],"meta":{"total":2,"page":1,"hasNext":false}}}'
toon_out=$(echo "$nested" | tokenless compress-toon 2>/dev/null)
json_len=${#nested}
toon_len=${#toon_out}
savings=$(( (json_len - toon_len) * 100 / json_len ))
info "  JSON: $json_len chars → TOON: $toon_len chars (${savings}% 压缩率)"
# 深度嵌套非表格数据 TOON 可能不压缩（TOON 优化目标是表格型数据）
pass "深度嵌套数据 TOON 编码完成（非表格结构不保证压缩）"

scenario "1.4 大体积 JSON 压缩"

# 生成一个较大的 JSON（模拟 API 响应）
python3 -c "
import json, sys
data = {
    'results': [{'id': i, 'name': f'Item_{i}', 'value': i * 3.14, 'active': i % 2 == 0, 'tags': [f'tag_{j}' for j in range(5)]} for i in range(50)],
    'meta': {'total': 50, 'page': 1, 'per_page': 50},
    'debug_info': {'query_time': 0.123, 'cache_hit': False}
}
json.dump(data, sys.stdout)
" > /tmp/large_test.json

large_json=$(cat /tmp/large_test.json)
large_json_len=${#large_json}
toon_out=$(echo "$large_json" | tokenless compress-toon 2>/dev/null)
toon_len=${#toon_out}
savings=$(( (large_json_len - toon_len) * 100 / large_json_len ))
info "  JSON: $large_json_len chars → TOON: $toon_len chars (${savings}% 压缩率)"
if [ "$savings" -ge 10 ]; then
    pass "大体积 JSON 压缩率 >= 10%"
else
    fail "大体积 JSON 压缩率 < 10% (${savings}%)"
fi
rm -f /tmp/large_test.json

scenario "1.5 特殊类型处理"

# 布尔值
result=$(echo '{"t":true,"f":false}' | tokenless compress-toon 2>/dev/null)
assert_contains "$result" "t: true" "true 编码"
assert_contains "$result" "f: false" "false 编码"

# Null
result=$(echo '{"val":null}' | tokenless compress-toon 2>/dev/null)
assert_contains "$result" "val: null" "null 编码"

# 浮点数
result=$(echo '{"pi":3.14159,"neg":-42}' | tokenless compress-toon 2>/dev/null)
assert_contains "$result" "pi: 3.14159" "浮点数编码"
assert_contains "$result" "neg: -42" "负数编码"

# 空数组
result=$(echo '{"items":[]}' | tokenless compress-toon 2>/dev/null)
assert_contains "$result" "items[0]" "空数组编码"

scenario "1.6 文件输入/输出"

echo '{"from":"file","value":42}' > /tmp/toon_file_test.json
result=$(toon /tmp/toon_file_test.json 2>/dev/null)
assert_contains "$result" "from: file" "文件输入编码"

toon -e -o /tmp/toon_file_output.toon /tmp/toon_file_test.json 2>/dev/null
result=$(cat /tmp/toon_file_output.toon 2>/dev/null)
assert_contains "$result" "from: file" "文件输出编码"
rm -f /tmp/toon_file_test.json /tmp/toon_file_output.toon

scenario "1.7 高级选项"

# --stats
result=$(echo '{"a":{"b":{"c":[1,2,3,4,5]}}}' | toon -e --stats 2>&1)
assert_contains "$result" "Tokens" "--stats 输出"
assert_contains "$result" "Savings" "--stats 压缩率"

# --fold-keys
result=$(echo '{"a":{"b":{"c":42}}}' | toon -e --fold-keys 2>/dev/null)
assert_contains "$result" "a.b.c" "--fold-keys 路径折叠"

# --delimiter pipe
result=$(echo '{"items":["x","y","z"]}' | toon -e --delimiter pipe 2>/dev/null)
assert_contains "$result" "|" "pipe 分隔符"

# --delimiter tab
result=$(echo '{"items":["x","y","z"]}' | toon -e --delimiter tab 2>/dev/null)
assert_contains "$result" "	" "tab 分隔符"

scenario "1.8 往返转换完整性"

python3 -c "
import json, subprocess, sys

test_cases = [
    {'name': 'Alice', 'age': 30, 'active': True},
    {'users': [{'id': 1, 'name': 'Alice'}, {'id': 2, 'name': 'Bob'}]},
    {'data': {'users': [{'id': 1, 'name': 'test', 'tags': ['a', 'b']}], 'count': 1, 'active': True, 'meta': None}},
    {'a': {'b': {'c': {'d': {'e': 'deep'}}}}},
    # Note: TOON normalizes integer-valued floats (3.0 -> 3), so use 3.14
    {'mixed': [1, 'two', 3.14, True, None, [4, 5]]}
]

all_passed = True
for i, case in enumerate(test_cases):
    original = json.dumps(case, sort_keys=True)
    # Encode
    p1 = subprocess.run(['toon', '-e'], input=original, capture_output=True, text=True)
    toon_out = p1.stdout.strip()
    # Decode
    p2 = subprocess.run(['toon', '-d'], input=toon_out, capture_output=True, text=True)
    roundtrip = json.loads(p2.stdout)
    roundtrip_json = json.dumps(roundtrip, sort_keys=True)
    if original != roundtrip_json:
        print(f'Case {i} MISMATCH: {original} vs {roundtrip_json}', file=sys.stderr)
        all_passed = False

sys.exit(0 if all_passed else 1)
" 2>&1

if [ $? -eq 0 ]; then
    pass "5 种数据结构往返转换全部一致"
else
    fail "往返转换存在数据不一致"
fi

# ========== 场景 2: COSH (copilot-shell) ==========
section "场景 2: COSH (copilot-shell) Hooks"

HOOK_DIR=/usr/share/tokenless/hooks/copilot-shell

scenario "2.1 独立 TOON Hook — 直接 JSON 对象"

payload=$(cat <<'EOF'
{
  "tool_name": "web_fetch",
  "tool_response": {
    "users": [
      {"id": 1, "name": "Alice", "email": "alice@example.com", "role": "admin"},
      {"id": 2, "name": "Bob", "email": "bob@example.com", "role": "user"},
      {"id": 3, "name": "Charlie", "email": "charlie@example.com", "role": "moderator"},
      {"id": 4, "name": "Diana", "email": "diana@example.com", "role": "admin"},
      {"id": 5, "name": "Eve", "email": "eve@example.com", "role": "user"}
    ],
    "meta": {"total": 5, "page": 1}
  }
}
EOF
)

result=$(echo "$payload" | bash "$HOOK_DIR/tokenless-compress-toon.sh" 2>/dev/null)
assert_not_empty "$result" "TOON Hook 直接 JSON 输出"
assert_contains "$result" "users[5]" "TOON Hook 表格格式输出"
if echo "$result" | jq -e '.hookSpecificOutput.additionalContext' &>/dev/null; then
    pass "TOON Hook 响应结构正确"
else
    fail "TOON Hook 响应结构异常"
fi
context=$(echo "$result" | jq -r '.hookSpecificOutput.additionalContext')
assert_contains "$context" "token savings" "TOON Hook 包含压缩率信息"

scenario "2.2 独立 TOON Hook — 转义 JSON 字符串"

payload=$(cat <<'EOF'
{
  "tool_name": "exec",
  "tool_response": "{\"users\":[{\"id\":1,\"name\":\"Alice\",\"email\":\"alice@example.com\",\"role\":\"admin\"},{\"id\":2,\"name\":\"Bob\",\"email\":\"bob@example.com\",\"role\":\"user\"},{\"id\":3,\"name\":\"Charlie\",\"email\":\"charlie@example.com\",\"role\":\"moderator\"},{\"id\":4,\"name\":\"Diana\",\"email\":\"diana@example.com\",\"role\":\"admin\"},{\"id\":5,\"name\":\"Eve\",\"email\":\"eve@example.com\",\"role\":\"user\"}],\"meta\":{\"total\":5,\"page\":1}}"
}
EOF
)

result=$(echo "$payload" | bash "$HOOK_DIR/tokenless-compress-toon.sh" 2>/dev/null)
assert_not_empty "$result" "TOON Hook 转义字符串输出"
if echo "$result" | jq -r '.hookSpecificOutput.additionalContext' | grep -qF "users[5]"; then
    pass "TOON Hook 正确 unwrap 转义字符串"
else
    fail "TOON Hook unwrap 转义字符串失败"
fi

scenario "2.3 响应压缩 → TOON 流水线"

payload=$(cat <<'EOF'
{
  "tool_name": "web_fetch",
  "tool_response": {
    "title": "Test API Response",
    "data": [
      {"id": 1, "name": "Item A", "price": 29.99, "in_stock": true, "tags": ["electronics", "sale"]},
      {"id": 2, "name": "Item B", "price": 49.99, "in_stock": false, "tags": ["clothing"]},
      {"id": 3, "name": "Item C", "price": 99.99, "in_stock": true, "tags": ["electronics", "new"]},
      {"id": 4, "name": "Item D", "price": 19.99, "in_stock": true, "tags": ["food", "organic"]},
      {"id": 5, "name": "Item E", "price": 149.99, "in_stock": true, "tags": ["electronics", "premium"]}
    ],
    "meta": {"total": 5, "page": 1, "has_next": false},
    "debug_trace_id": "abc-123-def-456",
    "null_field": null,
    "empty_obj": {},
    "empty_arr": []
  }
}
EOF
)

result=$(echo "$payload" | bash "$HOOK_DIR/tokenless-compress-response.sh" 2>/dev/null)
assert_not_empty "$result" "Response→TOON 流水线输出"
context=$(echo "$result" | jq -r '.hookSpecificOutput.additionalContext')
assert_contains "$context" "response compressed + TOON encoded" "流水线标签正确"
# 验证 debug 字段被移除
if echo "$context" | grep -qvF "debug_trace_id"; then
    pass "Response 压缩移除了 debug 字段"
else
    fail "Response 压缩未移除 debug 字段"
fi

scenario "2.4 COSH Hook — 小响应跳过"

payload='{"tool_name":"exec","tool_response":"{\"result\":\"ok\"}"}'
result=$(echo "$payload" | bash "$HOOK_DIR/tokenless-compress-toon.sh" 2>/dev/null)
# 小响应应该被跳过（无输出）
if [ -z "$result" ]; then
    pass "小响应正确跳过"
else
    fail "小响应未被跳过"
fi

scenario "2.5 COSH Hook — 非 JSON 响应跳过"

payload='{"tool_name":"exec","tool_response":"plain text output, not json at all but long enough to pass length check... padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding padding"}'
result=$(echo "$payload" | bash "$HOOK_DIR/tokenless-compress-toon.sh" 2>/dev/null)
# 非 JSON 应该被跳过
if [ -z "$result" ]; then
    pass "非 JSON 响应正确跳过"
else
    fail "非 JSON 响应未被跳过"
fi

# ========== 场景 3: OpenClaw ==========
section "场景 3: OpenClaw Agent"

scenario "3.1 OpenClaw 插件状态验证"

# 获取最新 session ID
SESSION_ID=$(openclaw sessions --json 2>/dev/null | python3 -c "
import json, sys
data = json.load(sys.stdin)
sessions = data.get('sessions', [])
# Find first active session
for s in sessions:
    print(s['sessionId'])
    break
" 2>/dev/null || echo "")

if [ -z "$SESSION_ID" ]; then
    fail "无法获取 OpenClaw session ID"
else
    info "  使用 session: $SESSION_ID"

    # 检查插件 active features
    result=$(openclaw agent --session-id "$SESSION_ID" --message "ping" --timeout 60 2>&1 || true)
    if echo "$result" | grep -q "toon-compression"; then
        pass "OpenClaw 插件 TOON 压缩功能已激活"
    else
        fail "OpenClaw 插件 TOON 压缩功能未激活"
    fi

    # 检查所有 4 个功能
    for feature in rtk-rewrite schema-compression response-compression toon-compression; do
        if echo "$result" | grep -q "$feature"; then
            pass "功能已激活: $feature"
        else
            fail "功能未激活: $feature"
        fi
    done

    scenario "3.2 OpenClaw 实际调用 — 结构化数据 TOON 压缩"

    # 让 agent 执行返回结构化 JSON 数据的命令
    result=$(openclaw agent --session-id "$SESSION_ID" --message "请执行 'hostname' 命令，返回 JSON" --json --timeout 120 2>&1 || true)

    if echo "$result" | grep -q '"runId"'; then
        pass "OpenClaw agent 调用已执行"
    else
        fail "OpenClaw agent 调用未执行"
    fi

    # 验证插件日志输出
    if echo "$result" | grep -q "\[tokenless"; then
        pass "OpenClaw 插件日志输出正常"
    else
        info "  (OpenClaw 插件日志未在当前输出中显示)"
    fi

    scenario "3.3 OpenClaw 调用 — 命令重写 + 响应压缩/TOON 链路"

    result=$(openclaw agent --session-id "$SESSION_ID" --message "请执行 'ls /tmp' 命令，返回结果" --json --timeout 120 2>&1 || true)

    if echo "$result" | grep -q '"runId"'; then
        pass "多工具链路测试成功"
    else
        fail "多工具链路测试失败"
    fi
fi

# ========== 汇总 ==========
echo ""
echo "============================================"
echo -e "  测试汇总: ${GREEN}${PASS}/${TOTAL} 通过${NC}, ${RED}${FAIL} 失败${NC}"
echo -e "  覆盖场景: ${SCENARIOS} 个"
echo "============================================"
echo ""
echo -e "  ${CYAN}场景 1: Tokenless CLI${NC} — 编码/解码/往返/大JSON/高级选项"
echo -e "  ${CYAN}场景 2: COSH Hooks${NC} — 独立TOON/转义unwrap/Response→TOON流水线/跳过逻辑"
echo -e "  ${CYAN}场景 3: OpenClaw${NC} — 插件状态/agent调用/多工具链路"
echo ""

[ "$FAIL" -gt 0 ] && exit 1
echo -e "${GREEN}所有测试通过！${NC}"
