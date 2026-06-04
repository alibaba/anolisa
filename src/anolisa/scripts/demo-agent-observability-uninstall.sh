#!/usr/bin/env bash
#
# demo-agent-observability-uninstall.sh — end-to-end smoke for
# `anolisa uninstall agent-observability` (P0-C transaction-backed
# uninstall).
#
# What this does:
#   1. Allocates a fresh tmpdir under /tmp/anolisa-uninstall-demo-XXXXXX.
#   2. Plants a *fake* AgentSight "binary" (a one-line shell script),
#      computes its sha256, writes an overlay DistributionIndex pointing
#      at it via file://, then runs the real `enable` path once so the
#      capability is `Installed` in InstalledState before we exercise
#      `uninstall`. Same seed shape as demo-agent-observability-disable.sh
#      so on-disk layout the smoke inspects matches what operators see.
#   3. Walks `uninstall --dry-run` to confirm the planner reports the
#      ANOLISA-owned binary as `remove`, then walks the real uninstall
#      execute path. Asserts:
#         * the agentsight binary under
#           `$DEMO_ROOT/usr/local/bin/agentsight` is unlinked,
#         * `installed.toml` no longer carries the capability or its
#            non-shared component,
#         * the central log gained `started` + `succeeded` records for
#            the operation,
#         * a transaction journal landed under `$DEMO_ROOT/var/lib/anolisa/journal/`.
#   4. Asserts the boundary P0-D draws explicitly: `uninstall` is no
#      longer gated — execute returns ok=true. (`purge` stays
#      NOT_IMPLEMENTED; that path is covered by unit tests.)
#
# Scope / non-goals (P0-C uninstall):
#   * Linux x86_64 only — same host gate as demo-agent-observability.sh
#     because we run a real `enable` first to seed state, and the
#     agentsight manifest pins requires_os = "linux" /
#     requires_arch = ["x86_64"].
#   * `purge` is explicitly out of scope: it is still gated by the
#     framework pending manifest-driven config/cache discovery and
#     surfaces as NOT_IMPLEMENTED. Unit tests in
#     commands::tier1::uninstall::tests cover that gate.
#   * `--force` is a wire stub today — no behavioral coverage here.
#
# After a successful run the tmpdir is left in place; its path is the
# last line of stdout so you can inspect `installed.toml`, the central
# log, the journal directory, and confirm
# `usr/local/bin/agentsight` no longer exists.

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
[demo-uninstall] refusing to run on $HOST_OS.

This smoke seeds InstalledState by running the real \`enable\` path
first (P0-C uninstall has nothing to do without an installed object).
That seed step needs the agentsight component manifest's
\`requires_os = "linux"\` precheck to pass. On $HOST_OS the planner
correctly marks the plan Blocked and execute_enable refuses to touch
the filesystem — there would be no Installed object for \`uninstall\`
to remove.

To dry-run the planner on this host instead (no install attempted),
run from $ANOLISA_DIR:

  cargo run -- uninstall agent-observability --dry-run --json

To run this smoke, retry on a Linux/x86_64 host (a Linux container or
VM counts).
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
[demo-uninstall] refusing to run on arch=$NORM_ARCH.

The agentsight component manifest pins requires_arch = ["x86_64"], so
even with a valid DistributionIndex entry the planner would mark the
seed \`enable\` plan Blocked. Without a successful seed there is
nothing for \`uninstall\` to operate on.

To dry-run the planner on this host (it will report blocked at the
agentsight.arch precheck and exit 0), run from $ANOLISA_DIR:

  cargo run -- uninstall agent-observability --dry-run --json

To run this smoke end-to-end, retry on a Linux/x86_64 host.
EOF
  exit 1
fi

# --- prerequisites -----------------------------------------------------------
if ! command -v jq >/dev/null 2>&1; then
  echo "[demo-uninstall] this script parses CLI JSON envelopes via jq, which was not found on PATH." >&2
  echo "[demo-uninstall] install jq (e.g. \`apt-get install -y jq\` / \`dnf install -y jq\`) and rerun." >&2
  exit 1
fi
if ! command -v sha256sum >/dev/null 2>&1; then
  echo "[demo-uninstall] sha256sum not found on PATH (coreutils). install it and rerun." >&2
  exit 1
fi

# --- workspace ---------------------------------------------------------------
DEMO_ROOT="$(mktemp -d "/tmp/anolisa-uninstall-demo-XXXXXX")"
echo "[demo-uninstall] DEMO_ROOT=$DEMO_ROOT"

ARTIFACT_DIR="$DEMO_ROOT/artifacts"
mkdir -p "$ARTIFACT_DIR"
ARTIFACT_PATH="$ARTIFACT_DIR/agentsight"

# Fake AgentSight binary; same shape as the other demos so the seed
# enable's sha256/mode checks all pass.
cat >"$ARTIFACT_PATH" <<'EOF'
#!/usr/bin/env bash
echo "fake-agentsight (anolisa uninstall-demo build) - args: $*"
EOF
chmod 0755 "$ARTIFACT_PATH"

ARTIFACT_SHA="$(sha256sum "$ARTIFACT_PATH" | awk '{print $1}')"
echo "[demo-uninstall] artifact sha256=$ARTIFACT_SHA"

OVERLAY_DIR="$DEMO_ROOT/etc/anolisa/manifests/distribution-index"
mkdir -p "$OVERLAY_DIR"
OVERLAY_INDEX="$OVERLAY_DIR/index.toml"
ARTIFACT_URL="file://$ARTIFACT_PATH"

# Overlay DistributionIndex. The version pin matches the agentsight
# component manifest at src/anolisa/manifests/runtime/agentsight.toml
# (version = "0.2.0"); if you bump the manifest you must bump this
# string too or the resolver will report a version mismatch and the
# seed enable will fail before uninstall runs.
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

echo "[demo-uninstall] overlay index at $OVERLAY_INDEX"

# Build once so the per-step `cargo run` invocations don't each pay
# the compile cost.
echo "[demo-uninstall] building anolisa-cli (debug)…"
(cd "$ANOLISA_DIR" && cargo build -q -p anolisa-cli)

run_cli() {
  # All CLI invocations share --install-mode system --prefix $DEMO_ROOT
  # so writes land in the tmpdir and the overlay distribution-index is
  # picked up. Same convention as the other demos.
  (cd "$ANOLISA_DIR" && cargo run -q -p anolisa-cli -- \
    --install-mode system --prefix "$DEMO_ROOT" "$@")
}

step() {
  echo
  echo "── [demo-uninstall] $1 ──"
}

# Wrap a CLI invocation, capture JSON, print it, surface code/reason on
# failure.
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
    echo "[demo-uninstall] $label FAILED — code=$CODE reason=$REASON exit=$RC" >&2
    echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
    exit 1
  fi
}

# --- seed: enable agent-observability so uninstall has something to do -------
step "seed: enable agent-observability --json"
capture_cli "enable" enable agent-observability --json
ENABLE_OUT="$OUT"
OP_ID_ENABLE="$(printf '%s' "$ENABLE_OUT" | jq -r '.data.operation_id // empty')"
if [ -z "$OP_ID_ENABLE" ]; then
  echo "[demo-uninstall] enable returned ok=true but no operation_id in .data — JSON shape changed?" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
echo "[demo-uninstall] seed enable operation_id=$OP_ID_ENABLE"

SEED_BIN="$DEMO_ROOT/usr/local/bin/agentsight"
SEED_STATE="$DEMO_ROOT/var/lib/anolisa/installed.toml"
SEED_LOG="$DEMO_ROOT/var/log/anolisa/central.jsonl"
JOURNAL_DIR="$DEMO_ROOT/var/lib/anolisa/journal"
BACKUP_ROOT="$DEMO_ROOT/var/lib/anolisa/backups"
if [ ! -x "$SEED_BIN" ]; then
  echo "[demo-uninstall] seed expected $SEED_BIN to exist and be executable" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ ! -f "$SEED_STATE" ]; then
  echo "[demo-uninstall] seed expected $SEED_STATE to exist" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- uninstall --dry-run: planner shows the binary as Remove -----------------
step "uninstall agent-observability --dry-run --json"
capture_cli "uninstall-dry-run" uninstall agent-observability --dry-run --json
DRY_OUT="$OUT"
DRY_PHASES="$(printf '%s' "$DRY_OUT" | jq -r '.data.phases | length')"
if [ "$DRY_PHASES" -lt 1 ]; then
  echo "[demo-uninstall] dry-run plan must contain at least one phase, got $DRY_PHASES" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
DRY_REMOVE_HIT="$(printf '%s' "$DRY_OUT" | jq -r --arg p "$SEED_BIN" '
  [.data.components[].files[]? | select(.path == $p and .action == "remove")] | length
')"
if [ "$DRY_REMOVE_HIT" != "1" ]; then
  echo "[demo-uninstall] dry-run plan must mark $SEED_BIN action=remove (got $DRY_REMOVE_HIT)" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
# Dry-run is read-only.
if [ ! -x "$SEED_BIN" ]; then
  echo "[demo-uninstall] dry-run must not unlink $SEED_BIN" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- uninstall (real) --------------------------------------------------------
step "uninstall agent-observability --json (execute)"
capture_cli "uninstall" uninstall agent-observability --json
EXEC_OUT="$OUT"
OP_ID_UNINSTALL="$(printf '%s' "$EXEC_OUT" | jq -r '.data.operation_id // empty')"
if [ -z "$OP_ID_UNINSTALL" ]; then
  echo "[demo-uninstall] uninstall returned ok=true but no operation_id in .data — JSON shape changed?" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
echo "[demo-uninstall] uninstall operation_id=$OP_ID_UNINSTALL"

# --- on-disk verification: the binary MUST be gone ---------------------------
if [ -e "$SEED_BIN" ]; then
  echo "[demo-uninstall] $SEED_BIN still exists after uninstall — execute did not unlink ANOLISA-owned files" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- state: capability + non-shared component must be removed ----------------
if [ ! -f "$SEED_STATE" ]; then
  echo "[demo-uninstall] $SEED_STATE missing after uninstall — state file must persist" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if grep -q 'name = "agent-observability"' "$SEED_STATE"; then
  echo "[demo-uninstall] capability 'agent-observability' still present in $SEED_STATE" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if grep -q 'name = "agentsight"' "$SEED_STATE"; then
  echo "[demo-uninstall] component 'agentsight' still present in $SEED_STATE" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- status: capability should now report not-installed ----------------------
step "status agent-observability --json"
capture_cli "status" status agent-observability --json
STATUS_OUT="$OUT"
STATUS_LEN="$(printf '%s' "$STATUS_OUT" | jq -r '.data.capabilities | length')"
if [ "$STATUS_LEN" != "0" ]; then
  echo "[demo-uninstall] expected 0 capabilities in status after uninstall, got $STATUS_LEN" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- list --enabled: agent-observability must NOT appear ---------------------
step "list --enabled --json"
capture_cli "list" list --enabled --json
LIST_OUT="$OUT"
HAS_AO="$(printf '%s' "$LIST_OUT" | jq -r '[.data.capabilities[] | select(.name == "agent-observability")] | length')"
if [ "$HAS_AO" != "0" ]; then
  echo "[demo-uninstall] agent-observability appeared in 'list --enabled' after uninstall (got $HAS_AO entries)" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- logs: started + succeeded for the uninstall op id -----------------------
step "logs --operation-id $OP_ID_UNINSTALL --json"
capture_cli "logs" logs --operation-id "$OP_ID_UNINSTALL" --json
LOGS_OUT="$OUT"
LOG_COUNT="$(printf '%s' "$LOGS_OUT" | jq -r '.data | length')"
if [ "$LOG_COUNT" -lt 2 ]; then
  echo "[demo-uninstall] expected at least 2 log records for uninstall op_id=$OP_ID_UNINSTALL, got $LOG_COUNT" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
LOG_STATUSES="$(printf '%s' "$LOGS_OUT" | jq -r '[.data[] | .status] | sort | join(",")')"
if [ "$LOG_STATUSES" != "started,succeeded" ]; then
  echo "[demo-uninstall] expected log statuses 'started,succeeded', got '$LOG_STATUSES'" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- transaction journal landed under state_dir/journal ----------------------
JOURNAL_FILE="$JOURNAL_DIR/${OP_ID_UNINSTALL}.journal.toml"
if [ ! -f "$JOURNAL_FILE" ]; then
  echo "[demo-uninstall] expected transaction journal at $JOURNAL_FILE" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- backup tree: the executor backed up the binary before deleting ----------
BACKUP_OP_DIR="$BACKUP_ROOT/$OP_ID_UNINSTALL"
if [ ! -d "$BACKUP_OP_DIR" ]; then
  echo "[demo-uninstall] expected backup dir at $BACKUP_OP_DIR" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
BACKUP_FILES="$(find "$BACKUP_OP_DIR" -maxdepth 1 -name '*.bak' -type f | wc -l | tr -d ' ')"
if [ "$BACKUP_FILES" -lt 1 ]; then
  echo "[demo-uninstall] expected at least one *.bak file under $BACKUP_OP_DIR" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

# --- repeated uninstall: capability already gone -> CAPABILITY_NOT_INSTALLED -
step "uninstall agent-observability --json (already gone — must be invalid argument)"
set +e
OUT="$(run_cli uninstall agent-observability --json)"
RC=$?
set -e
echo "$OUT"
OK2="$(printf '%s' "$OUT" | jq -r '.ok')"
CODE2="$(printf '%s' "$OUT" | jq -r '.error.code // "?"')"
if [ "$OK2" = "true" ]; then
  echo "[demo-uninstall] second uninstall returned ok=true; expected CAPABILITY_NOT_INSTALLED" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi
if [ "$CODE2" != "INVALID_ARGUMENT" ]; then
  echo "[demo-uninstall] second uninstall: expected error.code=INVALID_ARGUMENT, got '$CODE2' (rc=$RC)" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
fi

echo
echo "[demo-uninstall] SUCCESS"
echo "[demo-uninstall]   removed binary    : $SEED_BIN (gone)"
echo "[demo-uninstall]   installed state   : $SEED_STATE"
echo "[demo-uninstall]   central log       : $SEED_LOG"
echo "[demo-uninstall]   journal           : $JOURNAL_FILE"
echo "[demo-uninstall]   backup            : $BACKUP_OP_DIR"
echo "[demo-uninstall]   enable op_id      : $OP_ID_ENABLE"
echo "[demo-uninstall]   uninstall op_id   : $OP_ID_UNINSTALL"
echo
echo "[demo-uninstall] DEMO_ROOT preserved for inspection:"
echo "$DEMO_ROOT"
