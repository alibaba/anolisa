#!/usr/bin/env bash
# Copyright 2026 Alibaba Cloud
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     http://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

# One-shot runner for the tokenless benchmark suite.
#
#   ./run-benchmarks.sh            # build, tests, benches, compression report
#   ./run-benchmarks.sh --quick    # skip criterion benches (tests + report only)
#
# The criterion benches follow the report methodology: run this 3 times and
# average the per-benchmark medians (criterion itself uses 100 samples/bench).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

QUICK=0
[[ "${1:-}" == "--quick" ]] && QUICK=1

# Record source identity for traceability. Provenance covers everything the
# report attributes results to: git rev, full working-tree dirtiness
# (staged + unstaged + untracked via `status --porcelain`), the exact
# Cargo.lock and fixture bytes, and the rtk version. Hostname is deliberately
# NOT recorded — it leaks infrastructure naming into shareable artifacts.
sha256_of() {
    if command -v sha256sum > /dev/null 2>&1; then
        sha256sum "$@" 2>/dev/null | awk '{print $1}' | tail -1
    else
        shasum -a 256 "$@" 2>/dev/null | awk '{print $1}' | tail -1
    fi
}

IDENTITY_FILE="$SCRIPT_DIR/benchmark_identity.json"
GIT_REV=$(git -C "$SCRIPT_DIR" rev-parse --short HEAD 2>/dev/null || echo "unknown")
if [[ -n "$(git -C "$SCRIPT_DIR" status --porcelain 2>/dev/null)" ]]; then
    GIT_DIRTY=true
else
    GIT_DIRTY=false
fi
LOCK_SHA=$(sha256_of "$SCRIPT_DIR/Cargo.lock" || echo "unknown")
FIXTURES_SHA=$(cat "$SCRIPT_DIR"/fixtures/*.json 2>/dev/null | sha256_of /dev/stdin || echo "unknown")
RTK_BIN_PATH="${RTK_BIN:-$SCRIPT_DIR/../third_party/rtk/target/release/rtk}"
RTK_VERSION=$("$RTK_BIN_PATH" --version 2>/dev/null | head -1 || echo "unavailable")
TOKENLESS_VERSION=$(grep -m1 '^version' "$SCRIPT_DIR/../Cargo.toml" 2>/dev/null | sed 's/.*"\(.*\)".*/\1/' || echo "unknown")
cat > "$IDENTITY_FILE" <<EOF
{
  "git_rev": "$GIT_REV",
  "dirty": $GIT_DIRTY,
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "cargo_lock_sha256": "$LOCK_SHA",
  "fixtures_sha256": "$FIXTURES_SHA",
  "rtk_version": "$RTK_VERSION",
  "tokenless_workspace_version": "$TOKENLESS_VERSION"
}
EOF
echo "==> Source identity recorded: $IDENTITY_FILE"

echo "==> Building benchmark suite (release)"
cargo build --release

echo "==> Quality + adversarial tests (cargo test)"
cargo test --release

if [[ "$QUICK" -eq 0 ]]; then
    LOG_FILE="benchmark_output_$(date +%Y%m%d_%H%M%S).log"
    echo "==> Performance benchmarks (criterion, 100 samples each)"
    cargo bench 2>&1 | tee "$LOG_FILE"
    echo "==> Benchmark output saved to $LOG_FILE"
fi

echo "==> Compression-rate report (Rust in-process)"
cargo run --release --bin compression_rate

echo "==> Done. Criterion HTML reports under target/criterion/."
