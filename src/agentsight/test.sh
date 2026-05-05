#!/bin/bash
# AgentSight 单测覆盖率一键获取脚本
# 用法: ./test.sh [dashboard|rust|all]

set -e

ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
MODE="${1:-all}"

# 颜色
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

print_header() {
  echo ""
  echo -e "${BLUE}════════════════════════════════════════════════════════════${NC}"
  echo -e "${BLUE}  $1${NC}"
  echo -e "${BLUE}════════════════════════════════════════════════════════════${NC}"
  echo ""
}

# ─── Dashboard 前端覆盖率 ───────────────────────────────────────────────────────

run_dashboard() {
  print_header "Dashboard 前端单测覆盖率 (vitest + v8)"

  cd "$ROOT_DIR/dashboard"

  if [ ! -d "node_modules" ]; then
    echo -e "${YELLOW}安装依赖...${NC}"
    npm install --silent
  fi

  echo "运行测试并收集覆盖率..."
  echo ""
  node_modules/.bin/vitest run --coverage 2>/dev/null | grep -v "^$"

  cd "$ROOT_DIR"
}

# ─── Rust 后端覆盖率 ──────────────────────────────────────────────────────────

run_rust() {
  print_header "Rust 后端单测覆盖率 (cargo-tarpaulin)"

  cd "$ROOT_DIR"

  # 检查 tarpaulin 是否安装
  if ! command -v cargo-tarpaulin &>/dev/null; then
    echo -e "${YELLOW}安装 cargo-tarpaulin...${NC}"
    cargo install cargo-tarpaulin
  fi

  echo "运行测试并收集覆盖率（仅 src/ 目录）..."
  echo -e "${YELLOW}提示: 首次运行较慢（需重新编译），后续会利用缓存加速${NC}"
  echo ""

  START_TIME=$(date +%s)

  # 运行 tarpaulin
  # --skip-clean: 不清理构建缓存
  # --timeout 180: 单个测试超时时间
  # --jobs: 并行编译任务数
  # --exclude-files: 排除 BPF 和生成代码
  OUTPUT=$(cargo tarpaulin \
    --skip-clean \
    --timeout 180 \
    --jobs $(nproc) \
    --exclude-files "src/bpf/*" "target/*" \
    2>&1)

  END_TIME=$(date +%s)
  ELAPSED=$((END_TIME - START_TIME))

  # 提取 src/ 目录的覆盖率统计
  COVERED=$(echo "$OUTPUT" | grep "^src/" | awk -F'[ /]' '{split($NF, a, "/"); covered+=a[1]; total+=a[2]} END {print covered}')
  TOTAL=$(echo "$OUTPUT" | grep "^src/" | awk -F'[ /]' '{split($NF, a, "/"); total+=a[2]} END {print total}')

  if [ -n "$TOTAL" ] && [ "$TOTAL" -gt 0 ]; then
    RATE=$(awk "BEGIN {printf \"%.2f\", ($COVERED/$TOTAL)*100}")
    echo ""
    echo -e "${GREEN}Rust src/ 覆盖率: ${COVERED}/${TOTAL} = ${RATE}%${NC}"
  else
    # 回退：直接输出 tarpaulin 的汇总行
    echo "$OUTPUT" | tail -5
  fi

  echo -e "${YELLOW}耗时: ${ELAPSED}s${NC}"
  cd "$ROOT_DIR"
}

# ─── 汇总 ─────────────────────────────────────────────────────────────────────

run_all() {
  run_dashboard
  run_rust
  print_header "覆盖率获取完成"
}

# ─── 入口 ─────────────────────────────────────────────────────────────────────

case "$MODE" in
  dashboard|front|frontend)
    run_dashboard
    ;;
  rust|backend)
    run_rust
    ;;
  all|"")
    run_all
    ;;
  *)
    echo "用法: $0 [dashboard|rust|all]"
    echo "  dashboard  - 仅获取前端覆盖率"
    echo "  rust       - 仅获取 Rust 后端覆盖率"
    echo "  all        - 获取全部覆盖率（默认）"
    exit 1
    ;;
esac
