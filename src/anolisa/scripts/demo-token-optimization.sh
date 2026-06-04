#!/usr/bin/env bash
#
# demo-token-optimization.sh — end-to-end smoke for `anolisa enable
# token-optimization` (P1-G0 demo, tokenless component).
#
# What this does:
#   1. Allocates a fresh tmpdir under /tmp/anolisa-token-demo-XXXXXX.
#   2. Plants *fake* `tokenless` + `rtk` binaries (one-line shell scripts)
#      under $DEMO_ROOT/artifacts/stage, packs them into a tar.gz at
#      $DEMO_ROOT/artifacts/tokenless-0.3.2.tar.gz, computes the
#      tarball's sha256, and writes an overlay DistributionIndex
#      pointing at it via file://. The tokenless manifest declares two
#      install files (`{bindir}/tokenless` + `{libexecdir}/tokenless/rtk`),
#      so we need a multi-file backend — `tar_gz` is the only choice; the
#      `binary` backend errors with `BinaryRequiresSingleDest` on two
#      dests (see install_runner.rs).
#   3. Walks the read-only CLI surface (env, list, status, logs) and the
#      P1-F real-execute surface (enable --dry-run, enable, status, logs
#      --operation-id) using --install-mode system --prefix $DEMO_ROOT
#      so every write lands inside the demo tmpdir — nothing touches
#      /var/lib/anolisa or /usr/local/{bin,libexec} on the host.
#
# Scope / non-goals (P1-G0):
#   * Linux only. The tokenless component manifest's os precheck
#     requires linux; on non-Linux hosts the plan is Blocked before any
#     IO happens and the smoke cannot reach the happy path. The script
#     exits early with that explanation on darwin / freebsd / etc.
#   * x86_64 host only for this smoke. The tokenless manifest declares
#     `requires_arch = ["x86_64", "aarch64"]`, but for demo stability we
#     mirror the AgentSight smoke and hard-gate to x86_64 so the
#     overlay-index `arch` field always pins something the resolver
#     accepts and the script doesn't ship with a half-tested aarch64
#     path. Drop the aarch64 gate in a follow-up once it's exercised.
#   * The "binaries" we install are tiny shell scripts, not real
#     tokenless / rtk builds. The point is to exercise the orchestrator
#     (download cache → install runner tar_gz extraction →
#     installed-state → central log → install lock) on payloads whose
#     sha256 is known.
#   * No HTTPS, no signature verification, no rpm/deb backend. These
#     are P1-G follow-ups.
#
# After a successful run the tmpdir is left in place; its path is the
# last line of stdout so you can `ls $DEMO_ROOT/usr/local/bin/` and
# `cat $DEMO_ROOT/var/log/anolisa/central.jsonl` for inspection.

set -euo pipefail

# Resolve repo paths relative to the script's location so this runs the
# same whether invoked from src/anolisa, the repo root, or anywhere
# else (CI, a tmp checkout, etc.).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ANOLISA_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

# --- host gate: Linux only ---------------------------------------------------
HOST_OS="$(uname -s)"
if [ "$HOST_OS" != "Linux" ]; then
  cat >&2 <<EOF
[demo] refusing to run on $HOST_OS.

This smoke needs the real-execute path to complete, and that requires
the tokenless component manifest's \`requires_os = "linux"\` precheck
to pass. On $HOST_OS the planner correctly marks the plan Blocked and
execute_enable refuses to touch the filesystem.

To dry-run the planner on this host instead (no install attempted),
run from $ANOLISA_DIR:

  cargo run -- enable token-optimization --dry-run --json

To run this smoke, retry on a Linux host (a Linux container or VM
counts; tokenless does not require root for its file installs).
EOF
  exit 1
fi

# --- arch normalization for the overlay index entry --------------------------
RAW_ARCH="$(uname -m)"
case "$RAW_ARCH" in
  x86_64 | amd64) NORM_ARCH="x86_64" ;;
  aarch64 | arm64) NORM_ARCH="aarch64" ;;
  *) NORM_ARCH="$RAW_ARCH" ;;
esac

if [ "$NORM_ARCH" != "x86_64" ]; then
  cat >&2 <<EOF
[demo] refusing to run on arch=$NORM_ARCH.

Even though the tokenless component manifest lists aarch64 in
requires_arch, this demo hard-gates to x86_64 for now (mirrors the
AgentSight smoke). Letting the script continue would produce an
overlay-index entry pinned to aarch64 that has only been smoke-tested
on x86_64 — not what this demo is meant to show. The aarch64 gate
will lift once the path is exercised end-to-end.

To dry-run the planner on this host (it will report whatever the
tokenless prechecks say and exit 0), run from $ANOLISA_DIR:

  cargo run -- enable token-optimization --dry-run --json

To run this smoke end-to-end, retry on a Linux/x86_64 host (a Linux
container or VM counts).
EOF
  exit 1
fi

# --- prerequisites -----------------------------------------------------------
if ! command -v jq >/dev/null 2>&1; then
  echo "[demo] this script extracts operation_id from JSON output via jq, which was not found on PATH." >&2
  echo "[demo] install jq (e.g. \`apt-get install -y jq\` / \`dnf install -y jq\`) and rerun." >&2
  exit 1
fi
if ! command -v sha256sum >/dev/null 2>&1; then
  echo "[demo] sha256sum not found on PATH (coreutils). install it and rerun." >&2
  exit 1
fi
if ! command -v tar >/dev/null 2>&1; then
  echo "[demo] tar not found on PATH. install it and rerun." >&2
  exit 1
fi

# --- workspace ---------------------------------------------------------------
DEMO_ROOT="$(mktemp -d "/tmp/anolisa-token-demo-XXXXXX")"
echo "[demo] DEMO_ROOT=$DEMO_ROOT"

ARTIFACT_DIR="$DEMO_ROOT/artifacts"
STAGE_DIR="$ARTIFACT_DIR/stage"
mkdir -p "$STAGE_DIR/bin" "$STAGE_DIR/libexec"

# Fake tokenless + rtk binaries: runnable shell scripts whose only job is
# to print a recognisable banner. InstallRunner only cares that the
# tar.gz bytes round-trip with the expected sha256 and that each entry's
# basename matches the install.files dest basename (`tokenless`, `rtk`);
# the file contents are inert and the mode on disk is forced to 0755 by
# `write_dest_atomic` regardless of what's in the tar header.
cat >"$STAGE_DIR/bin/tokenless" <<'EOF'
#!/usr/bin/env bash
echo "fake-tokenless (anolisa demo build) - args: $*"
EOF
chmod 0755 "$STAGE_DIR/bin/tokenless"

cat >"$STAGE_DIR/libexec/rtk" <<'EOF'
#!/usr/bin/env bash
echo "fake-rtk (anolisa demo build) - args: $*"
EOF
chmod 0755 "$STAGE_DIR/libexec/rtk"

ARTIFACT_PATH="$ARTIFACT_DIR/tokenless-0.3.2.tar.gz"
# tar_gz backend in install_runner matches entries by basename (see
# `read_tar_gz_basenames` + `tar_gz_install_extracts_matching_basenames`
# unit test), so any in-tar path is fine as long as the leaf names line
# up with the manifest's dest basenames. Stage layout (`bin/tokenless`,
# `libexec/rtk`) is just for human readability.
( cd "$STAGE_DIR" && tar -czf "$ARTIFACT_PATH" bin/tokenless libexec/rtk )

ARTIFACT_SHA="$(sha256sum "$ARTIFACT_PATH" | awk '{print $1}')"
echo "[demo] artifact sha256=$ARTIFACT_SHA"

OVERLAY_DIR="$DEMO_ROOT/etc/anolisa/manifests/distribution-index"
mkdir -p "$OVERLAY_DIR"
OVERLAY_INDEX="$OVERLAY_DIR/index.toml"
ARTIFACT_URL="file://$ARTIFACT_PATH"

# Overlay DistributionIndex. The version pin matches the tokenless
# component manifest at src/anolisa/manifests/runtime/tokenless.toml
# (version = "0.3.2"); if you bump the manifest you must bump this
# string too or the resolver will report a version mismatch.
#
# Optional `libc` / `pkg_base` are intentionally omitted so the entry
# matches whatever the host env probe returns (the resolver treats
# absent selectors as wildcards).
cat >"$OVERLAY_INDEX" <<EOF
schema_version = 1
channel = "stable"
publisher = "anolisa-demo"

[[entries]]
component = "tokenless"
version = "0.3.2"
channel = "stable"
artifact_type = "tar_gz"
backend = "tar_gz"
url = "$ARTIFACT_URL"
os = "linux"
arch = "$NORM_ARCH"
install_modes = ["system"]
sha256 = "$ARTIFACT_SHA"
EOF

echo "[demo] overlay index at $OVERLAY_INDEX"

# Build once so the per-step `cargo run` invocations don't each pay
# the compile cost.
echo "[demo] building anolisa-cli (debug)…"
(cd "$ANOLISA_DIR" && cargo build -q -p anolisa-cli)

run_cli() {
  # All CLI invocations share --install-mode system --prefix $DEMO_ROOT
  # so writes land in the tmpdir and the overlay distribution-index is
  # picked up. We pipe through cargo run (debug build) instead of
  # invoking the binary directly so this works on a fresh checkout
  # without a prior install step.
  (cd "$ANOLISA_DIR" && cargo run -q -p anolisa-cli -- \
    --install-mode system --prefix "$DEMO_ROOT" "$@")
}

step() {
  echo
  echo "── [demo] $1 ──"
}

step "env --json"
run_cli env --json

step "enable token-optimization --dry-run --json"
run_cli enable token-optimization --dry-run --json

step "enable token-optimization --json"
# `set -e` would abort the script the moment `run_cli` returns non-zero,
# which means the carefully-crafted error-bucket diagnostic below would
# never run. Drop into `set +e` just for the capture so we always see the
# CLI's JSON envelope, then restore. ENABLE_RC carries the real exit
# code (1 = EXECUTION_FAILED, 2 = INVALID_ARGUMENT, 64 = NOT_IMPLEMENTED).
set +e
ENABLE_OUT="$(run_cli enable token-optimization --json)"
ENABLE_RC=$?
set -e
echo "$ENABLE_OUT"

OK="$(printf '%s' "$ENABLE_OUT" | jq -r '.ok')"
if [ "$OK" != "true" ]; then
  CODE="$(printf '%s' "$ENABLE_OUT" | jq -r '.error.code // "?"')"
  REASON="$(printf '%s' "$ENABLE_OUT" | jq -r '.error.reason // "?"')"
  echo "[demo] enable FAILED — code=$CODE reason=$REASON exit=$ENABLE_RC" >&2
  echo "[demo] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

OP_ID="$(printf '%s' "$ENABLE_OUT" | jq -r '.data.operation_id // empty')"
if [ -z "$OP_ID" ]; then
  echo "[demo] enable returned ok=true but no operation_id in .data — JSON shape changed?" >&2
  echo "[demo] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
echo "[demo] operation_id=$OP_ID"

step "status token-optimization --json"
run_cli status token-optimization --json

step "list --enabled --json"
run_cli list --enabled --json

step "logs --json"
run_cli logs --json

step "logs --operation-id $OP_ID --json"
LOGS_OUT="$(run_cli logs --operation-id "$OP_ID" --json)"
echo "$LOGS_OUT"

LOG_COUNT="$(printf '%s' "$LOGS_OUT" | jq -r '.data | length')"
if [ "$LOG_COUNT" -lt 2 ]; then
  echo "[demo] expected at least 2 log records for operation_id=$OP_ID, got $LOG_COUNT" >&2
  echo "[demo] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- on-disk verification ----------------------------------------------------
# tokenless lays down two files: the user-facing binary in bindir and an
# internal helper in libexecdir. In system mode the FHS layout puts
# libexec under /usr/local/libexec/anolisa (see anolisa-platform
# fs_layout.rs `fhs::LIBEXEC`), so under our prefix it's
# $DEMO_ROOT/usr/local/libexec/anolisa/tokenless/rtk — NOT under
# /usr/local/share/anolisa/libexec.
BIN_PATH="$DEMO_ROOT/usr/local/bin/tokenless"
RTK_PATH="$DEMO_ROOT/usr/local/libexec/anolisa/tokenless/rtk"
STATE_PATH="$DEMO_ROOT/var/lib/anolisa/installed.toml"
LOG_PATH="$DEMO_ROOT/var/log/anolisa/central.jsonl"

if [ ! -x "$BIN_PATH" ]; then
  echo "[demo] expected $BIN_PATH to exist and be executable" >&2
  echo "[demo] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ ! -x "$RTK_PATH" ]; then
  echo "[demo] expected $RTK_PATH to exist and be executable" >&2
  echo "[demo] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ ! -f "$STATE_PATH" ]; then
  echo "[demo] expected $STATE_PATH to exist" >&2
  echo "[demo] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ ! -f "$LOG_PATH" ]; then
  echo "[demo] expected $LOG_PATH to exist" >&2
  echo "[demo] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

echo
echo "[demo] SUCCESS"
echo "[demo]   installed binary : $BIN_PATH"
echo "[demo]   installed helper : $RTK_PATH"
echo "[demo]   installed state  : $STATE_PATH"
echo "[demo]   central log      : $LOG_PATH"
echo "[demo]   operation_id     : $OP_ID"
echo
echo "[demo] DEMO_ROOT preserved for inspection:"
echo "$DEMO_ROOT"
