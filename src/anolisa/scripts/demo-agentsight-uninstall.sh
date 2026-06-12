#!/usr/bin/env bash
#
# demo-agentsight-uninstall.sh — end-to-end smoke for the component
# lifecycle pair `anolisa install agentsight` (raw backend) followed by
# `anolisa uninstall agentsight` (transaction-backed teardown).
#
# What this does:
#   1. Allocates a fresh tmpdir under /tmp/anolisa-uninstall-demo-XXXXXX
#      and uses `--install-mode system --prefix $DEMO_ROOT/prefix` for
#      every CLI call, so all reads/writes (repo.toml, state, cache,
#      logs, installed files) stay inside the tmpdir.
#   2. Plants a *fake* AgentSight "binary" (a one-line shell script) in a
#      local raw repository, computes its sha256, writes a
#      DistributionIndex (`v1/index.toml`) pointing at it via a
#      repo-relative url, and writes a repo.toml under
#      `$PREFIX/etc/anolisa/repo.toml` whose raw base_url is the
#      file:// repo root. That etc-dir repo.toml is the first hit in the
#      CLI's discovery chain (user/site config → packaged → dev-tree →
#      embedded), so no host path is touched.
#   3. Seeds state by running the real `install agentsight --json` path:
#      index fetch, sha256-verified download, manifest-declared file
#      install ({bindir}/agentsight), state + central-log record.
#   4. Walks `uninstall --dry-run` to confirm the LifecyclePlan marks
#      the ANOLISA-owned binary as action=remove, then executes the real
#      uninstall and asserts:
#        * the binary under `$PREFIX/usr/local/bin/agentsight` is gone,
#        * `installed.toml` no longer carries the component,
#        * `status --json` reports an empty `.data.components` array,
#        * `list --json` (against a local catalog) does not report
#          agentsight as installed,
#        * a second `uninstall` fails with INVALID_ARGUMENT (exit 2).
#
# Scope / non-goals:
#   * Linux x86_64 only — the seed `install` must pass the agentsight
#     component manifest's environment pins (`requires_os = "linux"`,
#     `requires_arch = ["x86_64"]`), and the distribution-index entry is
#     written for the host os/arch.
#   * `--purge` is out of scope: it is still gated by the framework and
#     surfaces as NOT_IMPLEMENTED. Unit tests in
#     commands::tier1::uninstall::tests cover that gate.
#   * `--force` is a wire stub today — no behavioral coverage here.
#
# After a successful run the tmpdir is left in place; its path is the
# last line of stdout so you can inspect `installed.toml`, the central
# log, and confirm `prefix/usr/local/bin/agentsight` no longer exists.

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

This smoke seeds InstalledState by running the real \`install\` path
first (uninstall has nothing to do without an installed component).
That seed step needs the agentsight component manifest's
\`requires_os = "linux"\` environment pin to hold; on $HOST_OS the
seed install would not produce an Installed object for \`uninstall\`
to remove.

To preview the uninstall planner on this host instead (no install
attempted), run from $ANOLISA_DIR:

  cargo run -- uninstall agentsight --dry-run --json

To run this smoke, retry on a Linux/x86_64 host (a Linux container or
VM counts).
EOF
  exit 1
fi

# --- arch normalization for the distribution-index entry ----------------------
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
even with a valid DistributionIndex entry the seed \`install\` would
not succeed. Without a successful seed there is nothing for
\`uninstall\` to operate on.

To preview the uninstall planner on this host, run from $ANOLISA_DIR:

  cargo run -- uninstall agentsight --dry-run --json

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

# Isolated install prefix: FsLayout::system(Some(prefix)) rebases every
# ANOLISA-owned root under it ($PREFIX/usr/local/bin, $PREFIX/etc/anolisa,
# $PREFIX/var/lib/anolisa, $PREFIX/var/cache/anolisa, ...).
PREFIX="$DEMO_ROOT/prefix"
mkdir -p "$PREFIX"

# --- local raw repository: artifact + distribution index ----------------------
# Layout matches the raw backend convention: base_url points at the v1
# distribution root that contains index.toml; index rows with a relative
# url resolve against that same root.
REPO_V1="$DEMO_ROOT/repo/v1"
mkdir -p "$REPO_V1"
ARTIFACT_PATH="$REPO_V1/agentsight-bin"

# Fake AgentSight binary. artifact_type = "binary" means the downloaded
# file IS the installed binary; the agentsight manifest declares exactly
# one [[install.files]] dest ({bindir}/agentsight, mode 0755), which is
# all the raw binary backend needs.
cat >"$ARTIFACT_PATH" <<'EOF'
#!/usr/bin/env bash
echo "fake-agentsight (anolisa uninstall-demo build) - args: $*"
EOF
chmod 0755 "$ARTIFACT_PATH"

# install refuses index entries without sha256 (unverifiable artifact),
# so publish the real digest.
ARTIFACT_SHA="$(sha256sum "$ARTIFACT_PATH" | awk '{print $1}')"
echo "[demo-uninstall] artifact sha256=$ARTIFACT_SHA"

# DistributionIndex at the raw v1 root. The version pin matches the
# agentsight component manifest at src/anolisa/manifests/runtime/agentsight.toml
# (version = "0.2.0"); if you bump the manifest you must bump this string
# too or install will warn about a manifest/artifact version mismatch.
cat >"$REPO_V1/index.toml" <<EOF
schema_version = 1
channel = "stable"
publisher = "anolisa-demo"

[[entries]]
component = "agentsight"
version = "0.2.0"
channel = "stable"
artifact_type = "binary"
backend = "raw"
url = "agentsight-bin"
os = "linux"
arch = "$NORM_ARCH"
install_modes = ["system"]
sha256 = "$ARTIFACT_SHA"
EOF
echo "[demo-uninstall] distribution index at $REPO_V1/index.toml"

# --- repo.toml: point the raw backend at the local repo -----------------------
# RepoConfig discovery probes <etc_dir>/repo.toml first; with our prefix
# that is $PREFIX/etc/anolisa/repo.toml — fully script-controlled, no
# /etc pollution.
mkdir -p "$PREFIX/etc/anolisa"
cat >"$PREFIX/etc/anolisa/repo.toml" <<EOF
schema_version = 1
default_backend = "raw"

[backends.raw]
base_url = "file://$REPO_V1"
EOF
echo "[demo-uninstall] repo.toml at $PREFIX/etc/anolisa/repo.toml"

# --- local component catalog for `list` ---------------------------------------
# `list` fetches a JSON component catalog from $ANOLISA_CATALOG_URL (or
# [catalog].url in <etc_dir>/config.toml). Provide a local one so the
# final list assertion exercises a real catalog row for agentsight.
CATALOG_PATH="$DEMO_ROOT/catalog.json"
cat >"$CATALOG_PATH" <<EOF
{
  "schema_version": 1,
  "components": [
    {
      "name": "agentsight",
      "display_name": "AgentSight",
      "summary": "Agent observability demo entry",
      "category": "observability",
      "version": "0.2.0",
      "status": "available"
    }
  ]
}
EOF
export ANOLISA_CATALOG_URL="file://$CATALOG_PATH"
echo "[demo-uninstall] catalog at $CATALOG_PATH"

# Build once so the per-step `cargo run` invocations don't each pay
# the compile cost.
echo "[demo-uninstall] building anolisa-cli (debug)…"
(cd "$ANOLISA_DIR" && cargo build -q -p anolisa-cli)

run_cli() {
  # All CLI invocations share --install-mode system --prefix $PREFIX so
  # every read/write lands in the tmpdir and the etc-dir repo.toml above
  # is the first discovery hit.
  (cd "$ANOLISA_DIR" && cargo run -q -p anolisa-cli -- \
    --install-mode system --prefix "$PREFIX" "$@")
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

fail() {
  echo "[demo-uninstall] $1" >&2
  echo "[demo-uninstall] DEMO_ROOT preserved for inspection: $DEMO_ROOT" >&2
  exit 1
}

SEED_BIN="$PREFIX/usr/local/bin/agentsight"
SEED_STATE="$PREFIX/var/lib/anolisa/installed.toml"
SEED_LOG="$PREFIX/var/log/anolisa/central.jsonl"

# --- seed: install agentsight so uninstall has something to do ----------------
step "seed: install agentsight --json (raw backend)"
capture_cli "install" install agentsight --json
INSTALL_OUT="$OUT"
OP_ID_INSTALL="$(printf '%s' "$INSTALL_OUT" | jq -r '.data.operation_id // empty')"
if [ -z "$OP_ID_INSTALL" ]; then
  fail "install returned ok=true but no operation_id in .data — JSON shape changed?"
fi
FILES_INSTALLED="$(printf '%s' "$INSTALL_OUT" | jq -r '.data.files_installed | length')"
if [ "$FILES_INSTALLED" -lt 1 ]; then
  fail "install must report a non-empty .data.files_installed, got $FILES_INSTALLED"
fi
BIN_REPORTED="$(printf '%s' "$INSTALL_OUT" | jq -r --arg p "$SEED_BIN" \
  '[.data.files_installed[] | select(. == $p)] | length')"
if [ "$BIN_REPORTED" != "1" ]; then
  fail "install must report $SEED_BIN in .data.files_installed (got $BIN_REPORTED)"
fi
echo "[demo-uninstall] seed install operation_id=$OP_ID_INSTALL"

if [ ! -x "$SEED_BIN" ]; then
  fail "seed expected $SEED_BIN to exist and be executable"
fi
if [ ! -f "$SEED_STATE" ]; then
  fail "seed expected $SEED_STATE to exist"
fi

# --- uninstall --dry-run: LifecyclePlan shows the binary as remove ------------
step "uninstall agentsight --dry-run --json"
capture_cli "uninstall-dry-run" uninstall agentsight --dry-run --json
DRY_OUT="$OUT"
# Dry-run renders the LifecyclePlan: top-level keys are .data.component /
# .data.components / .data.phases.
DRY_COMPONENT="$(printf '%s' "$DRY_OUT" | jq -r '.data.component // empty')"
if [ "$DRY_COMPONENT" != "agentsight" ]; then
  fail "dry-run plan must target component 'agentsight', got '$DRY_COMPONENT'"
fi
DRY_PHASES="$(printf '%s' "$DRY_OUT" | jq -r '.data.phases | length')"
if [ "$DRY_PHASES" -lt 1 ]; then
  fail "dry-run plan must contain at least one phase, got $DRY_PHASES"
fi
DRY_REMOVE_HIT="$(printf '%s' "$DRY_OUT" | jq -r --arg p "$SEED_BIN" '
  [.data.components[].files[]? | select(.path == $p and .action == "remove")] | length
')"
if [ "$DRY_REMOVE_HIT" != "1" ]; then
  fail "dry-run plan must mark $SEED_BIN action=remove (got $DRY_REMOVE_HIT)"
fi
# Dry-run is read-only.
if [ ! -x "$SEED_BIN" ]; then
  fail "dry-run must not unlink $SEED_BIN"
fi

# --- uninstall (real) ----------------------------------------------------------
step "uninstall agentsight --json (execute)"
capture_cli "uninstall" uninstall agentsight --json
EXEC_OUT="$OUT"
OP_ID_UNINSTALL="$(printf '%s' "$EXEC_OUT" | jq -r '.data.operation_id // empty')"
if [ -z "$OP_ID_UNINSTALL" ]; then
  fail "uninstall returned ok=true but no operation_id in .data — JSON shape changed?"
fi
echo "[demo-uninstall] uninstall operation_id=$OP_ID_UNINSTALL"

# --- on-disk verification: the binary MUST be gone -----------------------------
if [ -e "$SEED_BIN" ]; then
  fail "$SEED_BIN still exists after uninstall — execute did not unlink ANOLISA-owned files"
fi

# --- state: the component object must be removed --------------------------------
if [ ! -f "$SEED_STATE" ]; then
  fail "$SEED_STATE missing after uninstall — state file must persist"
fi
if grep -q 'name = "agentsight"' "$SEED_STATE"; then
  fail "component 'agentsight' still present in $SEED_STATE"
fi

# --- status: no components left in state ----------------------------------------
step "status --json"
capture_cli "status" status --json
STATUS_OUT="$OUT"
STATUS_LEN="$(printf '%s' "$STATUS_OUT" | jq -r '.data.components | length')"
if [ "$STATUS_LEN" != "0" ]; then
  fail "expected .data.components to be an empty array after uninstall, got length $STATUS_LEN"
fi

# --- list: catalog row for agentsight must not read installed -------------------
step "list --json"
capture_cli "list" list --json
LIST_OUT="$OUT"
LIST_AGENTSIGHT="$(printf '%s' "$LIST_OUT" | jq -r \
  '[.data.components[] | select(.name == "agentsight")] | length')"
if [ "$LIST_AGENTSIGHT" -lt 1 ]; then
  fail "local catalog row for agentsight missing from 'list' output"
fi
LIST_INSTALLED="$(printf '%s' "$LIST_OUT" | jq -r \
  '[.data.components[] | select(.name == "agentsight" and .status == "installed")] | length')"
if [ "$LIST_INSTALLED" != "0" ]; then
  fail "agentsight must not report status=installed in 'list' after uninstall"
fi

# --- repeated uninstall: component already gone -> INVALID_ARGUMENT (exit 2) ----
step "uninstall agentsight --json (already gone — must be invalid argument)"
set +e
OUT="$(run_cli uninstall agentsight --json)"
RC=$?
set -e
echo "$OUT"
OK2="$(printf '%s' "$OUT" | jq -r '.ok')"
CODE2="$(printf '%s' "$OUT" | jq -r '.error.code // "?"')"
if [ "$OK2" = "true" ]; then
  fail "second uninstall returned ok=true; expected INVALID_ARGUMENT"
fi
if [ "$CODE2" != "INVALID_ARGUMENT" ]; then
  fail "second uninstall: expected error.code=INVALID_ARGUMENT, got '$CODE2' (rc=$RC)"
fi
if [ "$RC" != "2" ]; then
  fail "second uninstall: expected exit code 2 for INVALID_ARGUMENT, got $RC"
fi

echo
echo "[demo-uninstall] SUCCESS"
echo "[demo-uninstall]   removed binary    : $SEED_BIN (gone)"
echo "[demo-uninstall]   installed state   : $SEED_STATE"
echo "[demo-uninstall]   central log       : $SEED_LOG"
echo "[demo-uninstall]   install op_id     : $OP_ID_INSTALL"
echo "[demo-uninstall]   uninstall op_id   : $OP_ID_UNINSTALL"
echo
echo "[demo-uninstall] DEMO_ROOT preserved for inspection:"
echo "$DEMO_ROOT"
