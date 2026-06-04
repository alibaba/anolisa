#!/usr/bin/env bash
#
# demo-agent-observability-disable.sh — end-to-end smoke for
# `anolisa disable agent-observability` (P1-I logical / control-plane
# disable).
#
# What this does:
#   1. Allocates a fresh tmpdir under /tmp/anolisa-disable-demo-XXXXXX.
#   2. Plants a *fake* AgentSight "binary" (a one-line shell script),
#      computes its sha256, writes an overlay DistributionIndex pointing
#      at it via file://, then runs the real `enable` path once so the
#      capability is `Installed` in InstalledState before we exercise
#      `disable`. This mirrors demo-agent-observability.sh exactly so
#      the on-disk layout the disable smoke inspects matches the one
#      operators actually see in CI / on a fresh box.
#   3. Walks `disable` itself: first the active path (Installed →
#      Disabled), then the idempotent path (Disabled → Disabled, no
#      state mutation), plus the `status` / `list --enabled` / `logs`
#      reads that confirm the state and audit trail flipped correctly.
#   4. Asserts the teardown boundary P1-I draws explicitly: the installed
#      binary under `$DEMO_ROOT/usr/local/bin/agentsight` MUST still exist
#      after disable. Disable flips state, may run disable hooks, and may
#      stop owned services best-effort, but it does not remove installed
#      files, `installed.toml`, or the central log.
#
# Scope / non-goals (P1-I logical disable):
#   * Linux x86_64 only. Same host gate as demo-agent-observability.sh
#     because we have to run a real `enable` first to seed state, and
#     the agentsight manifest pins requires_os = "linux" and
#     requires_arch = ["x86_64"]. Without enable, there is nothing for
#     disable to operate on.
#   * Full lifecycle teardown (file removal, systemctl disable, unit-file
#     removal) is explicitly out of scope. The demo asserts the stable
#     boundary: after `disable` the on-disk artifacts are untouched. When
#     destructive teardown lands, this script will need a parallel /
#     replacement smoke that asserts file removal — until then, the
#     "files stay" check is the contract.
#   * No `--feature` / `--purge` coverage: both flags are explicit
#     `NOT_IMPLEMENTED` in the CLI today and are exercised by unit
#     tests in `commands::tier1::disable::tests`, not this smoke.
#
# After a successful run the tmpdir is left in place; its path is the
# last line of stdout so you can inspect `installed.toml`, the central
# log, and confirm that `usr/local/bin/agentsight` is still present.

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
[demo-disable] refusing to run on $HOST_OS.

This smoke seeds InstalledState by running the real \`enable\` path
first (P1-I disable has nothing to do without an installed object).
That seed step needs the agentsight component manifest's
\`requires_os = "linux"\` precheck to pass. On $HOST_OS the planner
correctly marks the plan Blocked and execute_enable refuses to touch
the filesystem — there would be no Installed object for \`disable\`
to flip.

To dry-run the planner on this host instead (no install attempted),
run from $ANOLISA_DIR:

  cargo run -- enable agent-observability --dry-run --json

To run this smoke, retry on a Linux/x86_64 host (a Linux container or
VM counts).
EOF
  exit 1
fi

# --- arch normalization for the overlay index entry --------------------------
# The normalizer accepts the canonical names used by DistributionIndex rows;
# the host gate below still limits this smoke to Linux/x86_64 today.
RAW_ARCH="$(uname -m)"
case "$RAW_ARCH" in
  x86_64 | amd64) NORM_ARCH="x86_64" ;;
  aarch64 | arm64) NORM_ARCH="aarch64" ;;
  *) NORM_ARCH="$RAW_ARCH" ;;
esac

if [ "$NORM_ARCH" != "x86_64" ]; then
  cat >&2 <<EOF
[demo-disable] refusing to run on arch=$NORM_ARCH.

The agentsight component manifest pins requires_arch = ["x86_64"], so
even with a valid DistributionIndex entry the planner would mark the
seed \`enable\` plan Blocked. Without a successful seed there is
nothing for \`disable\` to operate on.

To dry-run the planner on this host (it will report blocked at the
agentsight.arch precheck and exit 0), run from $ANOLISA_DIR:

  cargo run -- enable agent-observability --dry-run --json

To run this smoke end-to-end, retry on a Linux/x86_64 host.
EOF
  exit 1
fi

# --- prerequisites -----------------------------------------------------------
if ! command -v jq >/dev/null 2>&1; then
  echo "[demo-disable] this script parses CLI JSON envelopes via jq, which was not found on PATH." >&2
  echo "[demo-disable] install jq (e.g. \`apt-get install -y jq\` / \`dnf install -y jq\`) and rerun." >&2
  exit 1
fi
if ! command -v sha256sum >/dev/null 2>&1; then
  echo "[demo-disable] sha256sum not found on PATH (coreutils). install it and rerun." >&2
  exit 1
fi

# --- workspace ---------------------------------------------------------------
DEMO_ROOT="$(mktemp -d "/tmp/anolisa-disable-demo-XXXXXX")"
echo "[demo-disable] DEMO_ROOT=$DEMO_ROOT"

ARTIFACT_DIR="$DEMO_ROOT/artifacts"
mkdir -p "$ARTIFACT_DIR"
ARTIFACT_PATH="$ARTIFACT_DIR/agentsight"

# A fake AgentSight binary: a runnable shell script whose only job is
# to print a recognisable banner. Same shape as demo-agent-observability
# so the sha256 round-trip / mode 0755 contract InstallRunner asserts is
# satisfied — disable doesn't care about the bytes, but enable (the seed
# step) does.
cat >"$ARTIFACT_PATH" <<'EOF'
#!/usr/bin/env bash
echo "fake-agentsight (anolisa disable-demo build) - args: $*"
EOF
chmod 0755 "$ARTIFACT_PATH"

ARTIFACT_SHA="$(sha256sum "$ARTIFACT_PATH" | awk '{print $1}')"
echo "[demo-disable] artifact sha256=$ARTIFACT_SHA"

OVERLAY_DIR="$DEMO_ROOT/etc/anolisa/manifests/distribution-index"
mkdir -p "$OVERLAY_DIR"
OVERLAY_INDEX="$OVERLAY_DIR/index.toml"
ARTIFACT_URL="file://$ARTIFACT_PATH"

# Overlay DistributionIndex. The version pin matches the agentsight
# component manifest at src/anolisa/manifests/runtime/agentsight.toml
# (version = "0.2.0"); if you bump the manifest you must bump this
# string too or the resolver will report a version mismatch and the
# seed enable will fail before disable runs.
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

echo "[demo-disable] overlay index at $OVERLAY_INDEX"

# Build once so the per-step `cargo run` invocations don't each pay
# the compile cost.
echo "[demo-disable] building anolisa-cli (debug)…"
(cd "$ANOLISA_DIR" && cargo build -q -p anolisa-cli)

run_cli() {
  # All CLI invocations share --install-mode system --prefix $DEMO_ROOT
  # so writes land in the tmpdir and the overlay distribution-index is
  # picked up. Same convention as demo-agent-observability.sh.
  (cd "$ANOLISA_DIR" && cargo run -q -p anolisa-cli -- \
    --install-mode system --prefix "$DEMO_ROOT" "$@")
}

step() {
  echo
  echo "── [demo-disable] $1 ──"
}

# Wrap a CLI invocation, capture JSON, print it, surface code/reason on
# failure (set +e is necessary because `set -e` would abort before the
# jq diagnostic could run).
capture_cli() {
  local label="$1"
  shift
  set +e
  OUT="$(run_cli "$@")"
  RC=$?
  set -e
  echo "$OUT"
  OK="$(printf '%s' "$OUT" | jq -r '.ok')"
  if [ "$OK" != "true" ]; then
    CODE="$(printf '%s' "$OUT" | jq -r '.error.code // "?"')"
    REASON="$(printf '%s' "$OUT" | jq -r '.error.reason // "?"')"
    echo "[demo-disable] $label FAILED — code=$CODE reason=$REASON exit=$RC" >&2
    echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
    exit 1
  fi
}

# --- seed: enable agent-observability so disable has something to do ---------
step "seed: enable agent-observability --json"
capture_cli "enable" enable agent-observability --json
ENABLE_OUT="$OUT"
OP_ID_ENABLE="$(printf '%s' "$ENABLE_OUT" | jq -r '.data.operation_id // empty')"
if [ -z "$OP_ID_ENABLE" ]; then
  echo "[demo-disable] enable returned ok=true but no operation_id in .data — JSON shape changed?" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
echo "[demo-disable] seed enable operation_id=$OP_ID_ENABLE"

# Sanity-check the seed: agentsight binary exists, state file exists.
SEED_BIN="$DEMO_ROOT/usr/local/bin/agentsight"
SEED_STATE="$DEMO_ROOT/var/lib/anolisa/installed.toml"
SEED_LOG="$DEMO_ROOT/var/log/anolisa/central.jsonl"
if [ ! -x "$SEED_BIN" ]; then
  echo "[demo-disable] seed expected $SEED_BIN to exist and be executable" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ ! -f "$SEED_STATE" ]; then
  echo "[demo-disable] seed expected $SEED_STATE to exist" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- disable: active path (Installed → Disabled) -----------------------------
step "disable agent-observability --json (active path)"
capture_cli "disable" disable agent-observability --json
DISABLE_OUT="$OUT"
OP_ID_DISABLE="$(printf '%s' "$DISABLE_OUT" | jq -r '.data.operation_id // empty')"
if [ -z "$OP_ID_DISABLE" ]; then
  echo "[demo-disable] disable returned ok=true but no operation_id in .data — JSON shape changed?" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

PREV_STATUS="$(printf '%s' "$DISABLE_OUT" | jq -r '.data.previous_status')"
NEW_STATUS="$(printf '%s' "$DISABLE_OUT" | jq -r '.data.status')"
CHANGED="$(printf '%s' "$DISABLE_OUT" | jq -r '.data.changed')"
if [ "$PREV_STATUS" != "installed" ]; then
  echo "[demo-disable] expected previous_status=installed, got '$PREV_STATUS'" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ "$NEW_STATUS" != "disabled" ]; then
  echo "[demo-disable] expected status=disabled, got '$NEW_STATUS'" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ "$CHANGED" != "true" ]; then
  echo "[demo-disable] expected changed=true on first disable, got '$CHANGED'" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
echo "[demo-disable] disable operation_id=$OP_ID_DISABLE (previous=$PREV_STATUS, status=$NEW_STATUS, changed=$CHANGED)"

# --- status: capability should now report disabled ---------------------------
step "status agent-observability --json"
capture_cli "status" status agent-observability --json
STATUS_OUT="$OUT"
CUR_STATUS="$(printf '%s' "$STATUS_OUT" | jq -r '.data.capabilities[0].status // empty')"
if [ "$CUR_STATUS" != "disabled" ]; then
  echo "[demo-disable] expected status.data.capabilities[0].status=disabled, got '$CUR_STATUS'" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- list --enabled: agent-observability must NOT appear ---------------------
step "list --enabled --json"
capture_cli "list" list --enabled --json
LIST_OUT="$OUT"
HAS_AO="$(printf '%s' "$LIST_OUT" | jq -r '[.data.capabilities[] | select(.name == "agent-observability")] | length')"
if [ "$HAS_AO" != "0" ]; then
  echo "[demo-disable] agent-observability appeared in 'list --enabled' after disable (got $HAS_AO entries)" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- logs: started + succeeded for the disable op id -------------------------
step "logs --operation-id $OP_ID_DISABLE --json"
capture_cli "logs" logs --operation-id "$OP_ID_DISABLE" --json
LOGS_OUT="$OUT"
LOG_COUNT="$(printf '%s' "$LOGS_OUT" | jq -r '.data | length')"
if [ "$LOG_COUNT" -lt 2 ]; then
  echo "[demo-disable] expected at least 2 log records for disable op_id=$OP_ID_DISABLE, got $LOG_COUNT" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- disable: idempotent path (Disabled → Disabled, no state change) ---------
step "disable agent-observability --json (idempotent path)"
capture_cli "disable-again" disable agent-observability --json
DISABLE2_OUT="$OUT"
PREV2="$(printf '%s' "$DISABLE2_OUT" | jq -r '.data.previous_status')"
NEW2="$(printf '%s' "$DISABLE2_OUT" | jq -r '.data.status')"
CHANGED2="$(printf '%s' "$DISABLE2_OUT" | jq -r '.data.changed')"
if [ "$PREV2" != "disabled" ]; then
  echo "[demo-disable] idempotent path: expected previous_status=disabled, got '$PREV2'" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ "$NEW2" != "disabled" ]; then
  echo "[demo-disable] idempotent path: expected status=disabled, got '$NEW2'" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ "$CHANGED2" != "false" ]; then
  echo "[demo-disable] idempotent path: expected changed=false, got '$CHANGED2'" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- on-disk verification: P1-I MUST NOT delete files ------------------------
# This is the contract assertion that distinguishes logical disable from
# lifecycle disable. If a future change starts deleting files on disable
# without updating this assertion, the demo will catch it.
if [ ! -x "$SEED_BIN" ]; then
  echo "[demo-disable] $SEED_BIN was removed by disable — P1-I MUST NOT delete files" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ ! -f "$SEED_STATE" ]; then
  echo "[demo-disable] $SEED_STATE was removed by disable — state file must persist" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ ! -f "$SEED_LOG" ]; then
  echo "[demo-disable] $SEED_LOG missing — central log must persist" >&2
  echo "[demo-disable] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

echo
echo "[demo-disable] SUCCESS"
echo "[demo-disable]   retained binary  : $SEED_BIN"
echo "[demo-disable]   installed state  : $SEED_STATE"
echo "[demo-disable]   central log      : $SEED_LOG"
echo "[demo-disable]   enable op_id     : $OP_ID_ENABLE"
echo "[demo-disable]   disable op_id    : $OP_ID_DISABLE"
echo
echo "[demo-disable] DEMO_ROOT preserved for inspection:"
echo "$DEMO_ROOT"
