#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALLER="${SCRIPT_DIR}/../static/install.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/anolisa-installer-test.XXXXXX")"
FAKE_BIN="${TEST_ROOT}/bin"
TEST_CLI_VERSION="0.0.0-test"
REAL_TAR="$(command -v tar)"

cleanup() {
  rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

mkdir -p "$FAKE_BIN"

cat >"${FAKE_BIN}/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

output=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -o)
      output="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done

if [ -z "$output" ]; then
  test -s "$ANOLISA_TEST_SHA_FILE"
  printf '%s  artifact\n' "$(cat "$ANOLISA_TEST_SHA_FILE")"
  exit 0
fi

payload_dir="$(mktemp -d)"
trap 'rm -rf "$payload_dir"' EXIT
cat >"${payload_dir}/anolisa" <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail

if [ "${1:-}" = "--version" ]; then
  echo "anolisa ${ANOLISA_TEST_VERSION}"
  exit 0
fi
printf '%s\n' "$*" >>"$ANOLISA_TEST_LOG"
SCRIPT
chmod +x "${payload_dir}/anolisa"
"$ANOLISA_TEST_TAR" -czf "$output" -C "$payload_dir" anolisa
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$output" | awk '{print $1}' >"$ANOLISA_TEST_SHA_FILE"
else
  shasum -a 256 "$output" | awk '{print $1}' >"$ANOLISA_TEST_SHA_FILE"
fi
EOF

cat >"${FAKE_BIN}/uname" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -s) echo Darwin ;;
  -m) echo arm64 ;;
  *) exit 2 ;;
esac
EOF

cat >"${FAKE_BIN}/id" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -u) echo 1000 ;;
  *) exit 2 ;;
esac
EOF

cat >"${FAKE_BIN}/sudo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"$ANOLISA_TEST_SUDO_LOG"
cat >"${ANOLISA_INSTALL_DIR}/anolisa" <<'SCRIPT'
#!/usr/bin/env bash
printf 'tampered CLI executed\n' >"$ANOLISA_TEST_TAMPER_LOG"
exit 99
SCRIPT
chmod +x "${ANOLISA_INSTALL_DIR}/anolisa"
exec "$@"
EOF

chmod +x "${FAKE_BIN}/curl" "${FAKE_BIN}/uname" "${FAKE_BIN}/id" \
  "${FAKE_BIN}/sudo"

run_case() {
  local name="$1"
  local expected="$2"
  shift 2

  local case_root="${TEST_ROOT}/${name}"
  local install_dir="${case_root}/install"
  local command_log="${case_root}/commands.log"
  mkdir -p "$case_root"

  PATH="${FAKE_BIN}:${PATH}" \
    ANOLISA_INSTALL_DIR="$install_dir" \
    ANOLISA_TEST_LOG="$command_log" \
    ANOLISA_TEST_SUDO_LOG="${case_root}/sudo.log" \
    ANOLISA_TEST_TAMPER_LOG="${case_root}/tampered.log" \
    ANOLISA_TEST_SHA_FILE="${case_root}/artifact.sha256" \
    ANOLISA_TEST_TAR="$REAL_TAR" \
    ANOLISA_TEST_VERSION="$TEST_CLI_VERSION" \
    ANOLISA_VERSION="$TEST_CLI_VERSION" \
    bash -s -- "$@" <"$INSTALLER" >"${case_root}/stdout" 2>"${case_root}/stderr"

  local actual=""
  if [ -f "$command_log" ]; then
    actual="$(cat "$command_log")"
  fi
  if [ "$actual" != "$expected" ]; then
    echo "case '$name' invoked '$actual', expected '$expected'" >&2
    exit 1
  fi
  if [ -e "${case_root}/tampered.log" ]; then
    echo "case '$name' executed a replaced user-local CLI" >&2
    exit 1
  fi
}

run_case cli-only ""
run_case help "" --help
grep -Fq -- "--component NAME" "${TEST_ROOT}/help/stdout"
grep -Fq -- "--backend BACKEND" "${TEST_ROOT}/help/stdout"
grep -Fq -- "system uses sudo when needed" "${TEST_ROOT}/help/stdout"
run_case install-component-user \
  "--install-mode user install tokenless --backend raw" \
  --component tokenless --install-mode user
run_case install-component-equals "install agent-memory --backend raw" --component=agent-memory
run_case install-cosh-ng-alias "install cosh-ng --backend raw" --cosh-ng
run_case install-cosh-ng-system \
  "--install-mode system install cosh-ng --backend raw" \
  --cosh-ng --install-mode=system
test -s "${TEST_ROOT}/install-cosh-ng-system/sudo.log"
run_case install-cosh-ng-rpm \
  "--install-mode system install cosh-ng --backend rpm" \
  --cosh-ng --backend=rpm --install-mode=system
run_case upgrade-component \
  "--install-mode system update cosh-ng" \
  --component cosh-ng --install-mode system --upgrade
run_case uninstall-component \
  "--install-mode system uninstall cosh-ng" \
  --uninstall --component cosh-ng --install-mode system

expect_rejected() {
  local name="$1"
  local expected_error="$2"
  shift 2

  local case_root="${TEST_ROOT}/${name}"
  mkdir -p "$case_root"
  if PATH="${FAKE_BIN}:${PATH}" \
    ANOLISA_INSTALL_DIR="${case_root}/install" \
    ANOLISA_TEST_LOG="${case_root}/commands.log" \
    ANOLISA_TEST_SUDO_LOG="${case_root}/sudo.log" \
    ANOLISA_TEST_TAMPER_LOG="${case_root}/tampered.log" \
    ANOLISA_TEST_SHA_FILE="${case_root}/artifact.sha256" \
    ANOLISA_TEST_TAR="$REAL_TAR" \
    ANOLISA_TEST_VERSION="$TEST_CLI_VERSION" \
    ANOLISA_VERSION="$TEST_CLI_VERSION" \
    bash -s -- "$@" <"$INSTALLER" \
    >"${case_root}/stdout" 2>"${case_root}/stderr"; then
    echo "case '$name' unexpectedly succeeded" >&2
    exit 1
  fi

  if ! grep -Fq -- "$expected_error" "${case_root}/stderr"; then
    echo "case '$name' did not report '$expected_error'" >&2
    cat "${case_root}/stderr" >&2
    exit 1
  fi
}

expect_rejected conflicting-actions \
  "--upgrade and --uninstall cannot be used together" \
  --component cosh-ng --upgrade --uninstall
expect_rejected action-without-component \
  "--upgrade and --uninstall require --component NAME" \
  --upgrade
expect_rejected missing-component-name \
  "--component requires a component name" \
  --component
expect_rejected invalid-component-name \
  "invalid component name: invalid/name" \
  --component invalid/name
expect_rejected duplicate-component \
  "only one component can be selected" \
  --component cosh-ng --component tokenless
expect_rejected missing-install-mode \
  "--install-mode requires user or system" \
  --component cosh-ng --install-mode
expect_rejected invalid-install-mode \
  "invalid install mode: invalid (expected user or system)" \
  --component cosh-ng --install-mode invalid
expect_rejected install-mode-without-component \
  "--install-mode requires --component NAME" \
  --install-mode user
expect_rejected missing-backend-name \
  "--backend requires a backend name" \
  --component cosh-ng --backend
expect_rejected invalid-backend-name \
  "invalid backend: invalid/name" \
  --component cosh-ng --backend invalid/name
expect_rejected duplicate-backend \
  "only one component backend can be selected" \
  --component cosh-ng --backend raw --backend rpm
expect_rejected backend-without-component \
  "--backend requires --component NAME" \
  --backend rpm
expect_rejected backend-with-action \
  "--backend is only valid when installing a component" \
  --component cosh-ng --backend rpm --upgrade
expect_rejected unknown-argument \
  "unknown argument: --unknown" \
  --unknown

echo "installer tests passed"
