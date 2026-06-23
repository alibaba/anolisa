#!/usr/bin/env bash

set -euo pipefail

PROFILE="debug"
CARGO_FLAGS=""
for arg in "$@"; do
	case "$arg" in
		--release)
			PROFILE="release"
			CARGO_FLAGS="--release"
			;;
		*)
			echo "Unknown argument: $arg" >&2
			exit 1
			;;
	esac
done

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$REPO_ROOT/target/$PROFILE/skillfs"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/skillfs-e2e.XXXXXX")"
SOURCE_DIR="$TMP_ROOT/source"
MOUNT_DIR="$TMP_ROOT/mount"
PID_FILE="$TMP_ROOT/skillfs.pid"
LOG_FILE="$TMP_ROOT/skillfs.log"
MOUNT_PID=""

info() {
	echo "[skillfs-e2e] $1"
}

pass() {
	echo "[pass] $1"
}

fail() {
	echo "[fail] $1" >&2
	if [[ -f "$LOG_FILE" ]]; then
		echo "--- skillfs log ---" >&2
		cat "$LOG_FILE" >&2 || true
	fi
	exit 1
}

cleanup() {
	set +e
	if grep -Fq " $MOUNT_DIR " /proc/mounts 2>/dev/null; then
		fusermount3 -u "$MOUNT_DIR" >/dev/null 2>&1 || true
	fi
	if [[ -n "$MOUNT_PID" ]] && kill -0 "$MOUNT_PID" 2>/dev/null; then
		kill "$MOUNT_PID" 2>/dev/null || true
		wait "$MOUNT_PID" 2>/dev/null || true
	fi
	rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

assert_contains() {
	local haystack="$1"
	local needle="$2"
	local label="$3"

	if grep -Fq "$needle" <<<"$haystack"; then
		pass "$label"
	else
		fail "$label: missing '$needle'"
	fi
}

assert_not_contains() {
	local haystack="$1"
	local needle="$2"
	local label="$3"

	if grep -Fq "$needle" <<<"$haystack"; then
		fail "$label: unexpected '$needle'"
	else
		pass "$label"
	fi
}

assert_equals() {
	local actual="$1"
	local expected="$2"
	local label="$3"

	if [[ "$actual" == "$expected" ]]; then
		pass "$label"
	else
		fail "$label: expected '$expected', got '$actual'"
	fi
}

wait_for_mount_state() {
	local expected="$1"
	local attempts="${2:-50}"

	for _ in $(seq 1 "$attempts"); do
		if grep -Fq " $MOUNT_DIR " /proc/mounts 2>/dev/null; then
			[[ "$expected" == "mounted" ]] && return 0
		else
			[[ "$expected" == "unmounted" ]] && return 0
		fi
		sleep 0.1
	done
	return 1
}

if ! command -v fusermount3 >/dev/null 2>&1; then
	info "fusermount3 不存在，跳过端到端挂载测试"
	exit 0
fi

if [[ ! -e /dev/fuse ]]; then
	info "/dev/fuse 不存在，跳过端到端挂载测试"
	exit 0
fi

info "构建 skillfs ($PROFILE)"
cargo build $CARGO_FLAGS --bin skillfs --manifest-path "$REPO_ROOT/Cargo.toml" >/dev/null
[[ -x "$BIN" ]] || fail "二进制不存在: $BIN"

info "构造端到端测试数据"
mkdir -p "$SOURCE_DIR/primary-skill/assets" "$SOURCE_DIR/secondary-skill" "$SOURCE_DIR/tertiary-skill" "$MOUNT_DIR"

cat > "$SOURCE_DIR/primary-skill/SKILL.md" <<'EOF'
---
name: primary-skill
description: Primary skill exposed in the default view.
version: 1.0.0
tags: [primary]
enabled: true
---

# Primary Skill

This skill should appear directly under /skills.
EOF

cat > "$SOURCE_DIR/secondary-skill/SKILL.md" <<'EOF'
---
name: secondary-skill
description: Secondary skill only visible through skill-discover.
version: 1.0.0
tags: [secondary]
enabled: true
---

# Secondary Skill

This skill should only be referenced by skill-discover.
EOF

cat > "$SOURCE_DIR/tertiary-skill/SKILL.md" <<'EOF'
---
name: tertiary-skill
description: Another hidden skill to keep source_path relative to the source root.
version: 1.0.0
tags: [secondary]
enabled: true
---

# Tertiary Skill

This skill also lives in the secondary view.
EOF

cat > "$SOURCE_DIR/skillfs-views.toml" <<'EOF'
[[view]]
name = "major"
default = true
description = "Skills mounted by default"
skills = ["primary-skill"]

[[view]]
name = "other"
default = false
description = "Skills listed via skill-discover"
skills = ["secondary-skill", "tertiary-skill"]
EOF

printf 'passthrough-ok\n' > "$SOURCE_DIR/primary-skill/assets/info.txt"

info "启动 FUSE 挂载"
"$BIN" mount "$SOURCE_DIR" "$MOUNT_DIR" \
	--foreground \
	--pid-file "$PID_FILE" \
	--log-file "$LOG_FILE" \
	>/dev/null 2>&1 &
MOUNT_PID=$!

if ! wait_for_mount_state mounted; then
	fail "挂载超时"
fi
pass "FUSE 挂载成功"

ROOT_LIST="$(ls -1 "$MOUNT_DIR")"
assert_contains "$ROOT_LIST" "skills" "根目录暴露 skills"

SKILLS_LIST="$(ls -1 "$MOUNT_DIR/skills")"
assert_contains "$SKILLS_LIST" "primary-skill" "默认视图技能可见"
assert_contains "$SKILLS_LIST" "skill-discover" "skill-discover 始终可见"
assert_not_contains "$SKILLS_LIST" "secondary-skill" "secondary 技能不直接出现在 /skills"

PRIMARY_MD="$(cat "$MOUNT_DIR/skills/primary-skill/SKILL.md")"
assert_contains "$PRIMARY_MD" "name: primary-skill" "可读取 primary SKILL.md"

PASSTHROUGH_CONTENT="$(cat "$MOUNT_DIR/skills/primary-skill/assets/info.txt")"
assert_equals "$PASSTHROUGH_CONTENT" "passthrough-ok" "物理文件透传正确"

DISCOVER_MD="$(cat "$MOUNT_DIR/skills/skill-discover/SKILL.md")"
assert_contains "$DISCOVER_MD" "## other" "discover 包含 secondary view 章节"
assert_contains "$DISCOVER_MD" "secondary-skill" "discover 列出隐藏技能"
assert_contains "$DISCOVER_MD" "tertiary-skill" "discover 列出全部 secondary 技能"
assert_contains "$DISCOVER_MD" "| name | description | source_path |" "discover 暴露 source_path 列"
assert_contains "$DISCOVER_MD" "secondary-skill/SKILL.md" "discover source_path 相对路径正确"

info "发送 SIGTERM 触发卸载"
kill -TERM "$(cat "$PID_FILE")" >/dev/null 2>&1 || kill -TERM "$MOUNT_PID" >/dev/null 2>&1 || true
wait "$MOUNT_PID" 2>/dev/null || true
MOUNT_PID=""

if ! wait_for_mount_state unmounted; then
	fail "卸载超时"
fi
pass "FUSE 已正确卸载"

info "端到端测试完成"
