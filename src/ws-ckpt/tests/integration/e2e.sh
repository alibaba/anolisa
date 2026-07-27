#!/usr/bin/env bash
# ws-ckpt end-to-end integration test
#
# Requires: root, btrfs-progs, rsync
# Usage:    sudo bash tests/integration/e2e.sh
#
# Creates a temporary loop-btrfs filesystem, starts the daemon, exercises every
# CLI subcommand, and tears everything down.  Exit code 0 = all passed.

set -euo pipefail

# ── paths ─────────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CARGO_ROOT="$PROJECT_ROOT/src"

TMPBASE="$(mktemp -d /tmp/ws-ckpt-e2e.XXXXXX)"
IMG="$TMPBASE/btrfs.img"
MNT="$TMPBASE/mnt"
SOCKET="$TMPBASE/ws-ckpt.sock"
WORKSPACE="$TMPBASE/workspace"
DAEMON_PID=""
LOOP_DEV=""
STATE_DIR="/var/lib/ws-ckpt"
LOCKFILE="/run/ws-ckpt/ws-ckpt.lock"
STATE_DIR_BACKUP=""

PASS=0
FAIL=0

# ── helpers ───────────────────────────────────────────────────────────────────

cleanup() {
    echo ""
    echo "=== Cleanup ==="
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null && wait "$DAEMON_PID" 2>/dev/null || true
    umount "$MNT" 2>/dev/null || true
    [ -n "$LOOP_DEV" ] && losetup -d "$LOOP_DEV" 2>/dev/null || true
    # Clean test state from hardcoded state_dir
    rm -rf "$STATE_DIR"
    # Restore backed-up state_dir if it existed before test
    if [ -n "$STATE_DIR_BACKUP" ] && [ -d "$STATE_DIR_BACKUP" ]; then
        mv "$STATE_DIR_BACKUP" "$STATE_DIR"
        echo "Restored pre-existing $STATE_DIR"
    fi
    rm -rf "$TMPBASE"
    echo "Cleaned up $TMPBASE"
}

# Back up existing state_dir before cleanup can run, so early failures cannot
# remove real ws-ckpt state.
if [ -d "$STATE_DIR" ]; then
    STATE_DIR_BACKUP="$(mktemp -d /tmp/ws-ckpt-state-backup.XXXXXX)"
    mv "$STATE_DIR" "$STATE_DIR_BACKUP/state"
    STATE_DIR_BACKUP="$STATE_DIR_BACKUP/state"
    echo "Backed up existing $STATE_DIR to $STATE_DIR_BACKUP"
fi
trap cleanup EXIT

assert_ok() {
    local desc="$1"; shift
    if "$@" >/dev/null 2>&1; then
        echo "  PASS  $desc"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  $desc (exit $?)"
        FAIL=$((FAIL + 1))
    fi
}

assert_fail() {
    local desc="$1"; shift
    if "$@" >/dev/null 2>&1; then
        echo "  FAIL  $desc (expected failure, got success)"
        FAIL=$((FAIL + 1))
    else
        echo "  PASS  $desc"
        PASS=$((PASS + 1))
    fi
}

assert_output_contains() {
    local desc="$1"; shift
    local pattern="$1"; shift
    local output
    output=$("$@" 2>&1) || true
    if echo "$output" | grep -qi "$pattern"; then
        echo "  PASS  $desc"
        PASS=$((PASS + 1))
    else
        echo "  FAIL  $desc (output missing '$pattern')"
        FAIL=$((FAIL + 1))
    fi
}

# ── build ─────────────────────────────────────────────────────────────────────

echo "=== Building ws-ckpt ==="
cd "$CARGO_ROOT"
cargo build --release --workspace
BIN="$CARGO_ROOT/target/release/ws-ckpt"
[ -x "$BIN" ] || { echo "FATAL: binary not found at $BIN"; exit 1; }

# ── setup btrfs loop ──────────────────────────────────────────────────────────

echo "=== Setting up btrfs loop device ==="
dd if=/dev/zero of="$IMG" bs=1M count=256 status=none
LOOP_DEV=$(losetup --find --show "$IMG")
mkfs.btrfs -f "$LOOP_DEV" >/dev/null 2>&1
mkdir -p "$MNT" "$WORKSPACE"
mount "$LOOP_DEV" "$MNT"
echo "Loop device $LOOP_DEV mounted on $MNT"

# ── guard against running daemon ───────────────────────────────────────────────

if [ -S "/run/ws-ckpt/ws-ckpt.sock" ]; then
    echo "FATAL: a ws-ckpt daemon is already running (socket exists at /run/ws-ckpt/ws-ckpt.sock)."
    echo "Stop it first: systemctl stop ws-ckpt"
    exit 1
fi

# ── start daemon ──────────────────────────────────────────────────────────────

echo "=== Starting daemon ==="
"$BIN" daemon --mount-path "$MNT" --socket "$SOCKET" >"$TMPBASE/daemon.log" 2>&1 &
DAEMON_PID=$!
# Wait for socket
for i in $(seq 1 30); do
    [ -S "$SOCKET" ] && break
    sleep 0.2
done
[ -S "$SOCKET" ] || { echo "FATAL: daemon socket not ready after 6s"; exit 1; }
echo "Daemon started (PID $DAEMON_PID)"

# ── set WS_CKPT env for CLI ──────────────────────────────────────────────────

export WS_CKPT_SOCKET="$SOCKET"

# ── populate workspace ────────────────────────────────────────────────────────

echo "hello world" > "$WORKSPACE/file1.txt"
mkdir -p "$WORKSPACE/subdir"
echo "nested" > "$WORKSPACE/subdir/file2.txt"

# ── tests ─────────────────────────────────────────────────────────────────────

echo ""
echo "=== CLI Tests ==="

# status
assert_ok "status" "$BIN" status

# init
assert_ok "init workspace" "$BIN" init -w "$WORKSPACE"

# checkpoint
assert_ok "create checkpoint snap1" "$BIN" checkpoint -w "$WORKSPACE" -i snap1 -m "first snapshot"

# list
assert_ok "list snapshots" "$BIN" list -w "$WORKSPACE"
assert_output_contains "list shows snap1" "snap1" "$BIN" list -w "$WORKSPACE"

# modify workspace and create second checkpoint
echo "modified" >> "$WORKSPACE/file1.txt"
assert_ok "create checkpoint snap2" "$BIN" checkpoint -w "$WORKSPACE" -i snap2 -m "after modification"

# diff
assert_ok "diff snap1 snap2" "$BIN" diff -w "$WORKSPACE" --from snap1 --to snap2

# rollback
assert_ok "rollback to snap1" "$BIN" rollback -w "$WORKSPACE" -s snap1
CONTENT=$(cat "$WORKSPACE/file1.txt")
if [ "$CONTENT" = "hello world" ]; then
    echo "  PASS  rollback restored file content"
    PASS=$((PASS + 1))
else
    echo "  FAIL  rollback content mismatch: got '$CONTENT'"
    FAIL=$((FAIL + 1))
fi

# delete
assert_ok "delete snap2" "$BIN" delete -s snap2 -w "$WORKSPACE" --force

# list after delete
assert_output_contains "list after delete shows snap1" "snap1" "$BIN" list -w "$WORKSPACE"

# config view
assert_ok "config view" "$BIN" config -w "$WORKSPACE"

# cleanup
assert_ok "cleanup" "$BIN" cleanup -w "$WORKSPACE"

# error paths
assert_fail "init nonexistent" "$BIN" init -w /nonexistent/path/should/fail
assert_fail "checkpoint without init" "$BIN" checkpoint -w /tmp -i bad

# ── summary ───────────────────────────────────────────────────────────────────

echo ""
echo "=== Results ==="
echo "  Passed: $PASS"
echo "  Failed: $FAIL"
echo "  Total:  $((PASS + FAIL))"

if [ "$FAIL" -ne 0 ]; then
    echo ""
    echo "=== Daemon log (last 50 lines) ==="
    tail -50 "$TMPBASE/daemon.log" 2>/dev/null || true
    exit 1
fi
