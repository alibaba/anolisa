#!/usr/bin/env bash
#
# demo-agent-observability.sh — end-to-end smoke for `anolisa enable
# agent-observability` (P1-G0 demo).
#
# What this does:
#   1. Allocates a fresh tmpdir under /tmp/anolisa-demo-XXXXXX.
#   2. Plants a *fake* AgentSight "binary" (a one-line shell script) at
#      $DEMO_ROOT/artifacts/agentsight, computes its sha256, and writes
#      an overlay DistributionIndex pointing at it via file://.
#   3. Walks the read-only CLI surface (env, list, status, logs) and the
#      P1-F real-execute surface (enable --dry-run, enable, status, logs
#      --operation-id) using --install-mode system --prefix $DEMO_ROOT
#      so every write lands inside the demo tmpdir — nothing touches
#      /var/lib/anolisa or /usr/local/bin on the host.
#
# Scope / non-goals (P1-G0):
#   * Linux only. The agentsight component manifest's os precheck
#     requires linux; on non-Linux hosts the plan is Blocked before any
#     IO happens and the smoke cannot reach the happy path. The script
#     exits early with that explanation on darwin / freebsd / etc.
#   * x86_64 host recommended. The component manifest's
#     requires_arch=["x86_64"] gate makes aarch64 hosts hit Blocked
#     even though the DistributionIndex entry would otherwise resolve.
#     The script writes a normalized-arch entry either way; on aarch64
#     it prints the precheck explanation so the failure is legible.
#   * The "binary" we install is a tiny shell script, not a real
#     AgentSight build. The point is to exercise the orchestrator
#     (download cache → install runner → installed-state → central
#     log → install lock) on a payload whose sha256 is known.
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
the agentsight component manifest's \`requires_os = "linux"\` precheck
to pass. On $HOST_OS the planner correctly marks the plan Blocked and
execute_enable refuses to touch the filesystem.

To dry-run the planner on this host instead (no install attempted),
run from $ANOLISA_DIR:

  cargo run -- enable agent-observability --dry-run --json

To run this smoke, retry on a Linux host (a Linux container or VM
counts — root not required, but CAP_BPF probe being absent will
surface as a degraded plan rather than a hard block).
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

The agentsight component manifest pins requires_arch = ["x86_64"], so
even with a valid DistributionIndex entry the planner would mark the
plan Blocked and execute_enable would refuse to install. Letting the
script continue would just produce a misleading "happy-path smoke
fails at the install step" run — not what this demo is meant to show.

To dry-run the planner on this host (it will report blocked at the
agentsight.arch precheck and exit 0), run from $ANOLISA_DIR:

  cargo run -- enable agent-observability --dry-run --json

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

# --- workspace ---------------------------------------------------------------
DEMO_ROOT="$(mktemp -d "/tmp/anolisa-demo-XXXXXX")"
echo "[demo] DEMO_ROOT=$DEMO_ROOT"

ARTIFACT_DIR="$DEMO_ROOT/artifacts"
mkdir -p "$ARTIFACT_DIR"
ARTIFACT_PATH="$ARTIFACT_DIR/agentsight"

# A fake AgentSight binary: a runnable shell script whose only job is
# to print a recognisable banner. InstallRunner only cares that the
# bytes round-trip with the expected sha256 and the destination ends
# up mode 0755; the contents are inert.
cat >"$ARTIFACT_PATH" <<'EOF'
#!/usr/bin/env bash
echo "fake-agentsight (anolisa demo build) - args: $*"
EOF
chmod 0755 "$ARTIFACT_PATH"

ARTIFACT_SHA="$(sha256sum "$ARTIFACT_PATH" | awk '{print $1}')"
echo "[demo] artifact sha256=$ARTIFACT_SHA"

OVERLAY_DIR="$DEMO_ROOT/etc/anolisa/manifests/distribution-index"
mkdir -p "$OVERLAY_DIR"
OVERLAY_INDEX="$OVERLAY_DIR/index.toml"
ARTIFACT_URL="file://$ARTIFACT_PATH"

# Overlay DistributionIndex. The version pin matches the agentsight
# component manifest at src/anolisa/manifests/runtime/agentsight.toml
# (version = "0.2.0"); if you bump the manifest you must bump this
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
component = "agentsight"
version = "0.2.0"
channel = "stable"
artifact_type = "binary"
backend = "binary"
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

step "enable agent-observability --dry-run --json"
run_cli enable agent-observability --dry-run --json

step "enable agent-observability --json"
# `set -e` would abort the script the moment `run_cli` returns non-zero,
# which means the carefully-crafted error-bucket diagnostic below would
# never run. Drop into `set +e` just for the capture so we always see the
# CLI's JSON envelope, then restore. ENABLE_RC carries the real exit
# code (1 = EXECUTION_FAILED, 2 = INVALID_ARGUMENT, 64 = NOT_IMPLEMENTED).
set +e
ENABLE_OUT="$(run_cli enable agent-observability --json)"
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

step "status agent-observability --json"
run_cli status agent-observability --json

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
BIN_PATH="$DEMO_ROOT/usr/local/bin/agentsight"
STATE_PATH="$DEMO_ROOT/var/lib/anolisa/installed.toml"
LOG_PATH="$DEMO_ROOT/var/log/anolisa/central.jsonl"

if [ ! -x "$BIN_PATH" ]; then
  echo "[demo] expected $BIN_PATH to exist and be executable" >&2
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
echo "[demo]   installed state  : $STATE_PATH"
echo "[demo]   central log      : $LOG_PATH"
echo "[demo]   operation_id     : $OP_ID"
echo
echo "[demo] DEMO_ROOT preserved for inspection:"
echo "$DEMO_ROOT"
