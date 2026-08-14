#!/usr/bin/env bash
# Build, install, and exercise the AgentScope integration with its real dependencies.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEST_ROOT="$(mktemp -d /tmp/tokenless-agentscope-integration-test.XXXXXX)"
trap 'rm -rf "$TEST_ROOT"' EXIT

mapfile -t RUNTIME_WHEELS < <(find "$ROOT/target/wheels" -maxdepth 1 -type f \
    -name 'anolisa_tokenless-*.whl' -print)
mapfile -t INTEGRATION_WHEELS < <(find "$ROOT/target/wheels" -maxdepth 1 -type f \
    -name 'anolisa_tokenless_agentscope-*.whl' -print)
if [ "${#RUNTIME_WHEELS[@]}" -ne 1 ] || [ "${#INTEGRATION_WHEELS[@]}" -ne 1 ]; then
    echo "ERROR: expected exactly one runtime wheel and one AgentScope wheel" >&2
    exit 1
fi

run_smoke() {
    local venv_name="$1"
    local agentscope_requirement="$2"
    local venv="$TEST_ROOT/$venv_name"

    uv venv --python python3 "$venv" >/dev/null
    uv pip install --python "$venv/bin/python" \
        "$agentscope_requirement" \
        "${RUNTIME_WHEELS[0]}" "${INTEGRATION_WHEELS[0]}" >/dev/null
    "$venv/bin/python" "$ROOT/tests/agentscope_integration_smoke.py"
    "$venv/bin/python" - <<'PY'
import agentscope

print(f"AgentScope {agentscope.__version__} smoke test passed")
PY
}

run_smoke minimum 'agentscope==2.0.5'
run_smoke latest 'agentscope>=2.0.5,<2.1'

EXPECTED_VERSION="$(sed -n \
    's/^version = "\([^"]*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
"$TEST_ROOT/minimum/bin/python" - "$EXPECTED_VERSION" "${INTEGRATION_WHEELS[0]}" <<'PY'
from importlib.metadata import distribution
from pathlib import Path
import sys

package = distribution("anolisa-tokenless-agentscope")
assert package.version == distribution("anolisa-tokenless").version
assert package.version == sys.argv[1]
assert package.metadata.get_all("License-File") == ["LICENSE"]
license_paths = [
    path for path in package.files or ()
    if str(path).endswith(".dist-info/licenses/LICENSE")
]
assert len(license_paths) == 1
assert "Apache License" in Path(license_paths[0].locate()).read_text()
assert Path(sys.argv[2]).is_file()
PY

printf 'Tokenless AgentScope integration smoke test passed\n'
