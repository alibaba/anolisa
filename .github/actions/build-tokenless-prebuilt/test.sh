#!/usr/bin/env bash
# Exercise Tokenless action input binding and shared multi-project SBOM generation.
set -euo pipefail

ACTION_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMMON_DIR="$(cd "$ACTION_DIR/../prebuilt-rust-common" && pwd)"
REPO_ROOT="$(git -C "$ACTION_DIR" rev-parse --show-toplevel)"
TEMPORARY="$(mktemp -d)"
trap 'rm -rf -- "$TEMPORARY"' EXIT

python3 - "$ACTION_DIR/action.yaml" <<'PY'
import sys
from pathlib import Path


action = Path(sys.argv[1]).read_text(encoding="utf-8")
expected_bindings = (
    "TOKENLESS_VERSION: ${{ inputs.version }}",
    "TOKENLESS_TARGET_OS: ${{ inputs.target-os }}",
    "TOKENLESS_TARGET_ARCH: ${{ inputs.target-arch }}",
    "TOKENLESS_PROFILE: ${{ inputs.profile }}",
    "TOKENLESS_TAG: ${{ inputs.tag }}",
)
for binding in expected_bindings:
    if binding not in action:
        raise SystemExit(f"composite action is missing environment binding: {binding}")

marker = "      run: |\n"
if marker not in action:
    raise SystemExit("composite action is missing its Bash run block")
run_block = action.split(marker, 1)[1]
if "${{ inputs." in run_block:
    raise SystemExit("composite action interpolates an input directly into Bash")
PY

python3 - "$REPO_ROOT/.github/actions/package-source/action.yaml" <<'PY'
import sys
from pathlib import Path


action = Path(sys.argv[1]).read_text(encoding="utf-8")
if "bash scripts/setup-rtk.sh third_party/rtk" not in action:
    raise SystemExit("source packaging does not use the pinned RTK setup script")
if "RTK_TAG=" in action or "git clone --depth 1 --branch" in action:
    raise SystemExit("source packaging still resolves RTK through a mutable tag")
PY

RTK_SETUP="$REPO_ROOT/src/tokenless/scripts/setup-rtk.sh"
PINNED_RTK_COMMIT="$(
    sed -n 's/^RTK_COMMIT="\([0-9a-f]\{40\}\)"$/\1/p' "$RTK_SETUP"
)"
[ -n "$PINNED_RTK_COMMIT" ] || {
    printf 'ERROR: RTK setup script has no pinned 40-character commit\n' >&2
    exit 1
}
RTK_DRIFT="$TEMPORARY/rtk-drift"
install -d -m 0755 "$RTK_DRIFT"
printf '[package]\nname = "rtk"\nversion = "0.0.0"\n' > "$RTK_DRIFT/Cargo.toml"
printf '%040d\n' 0 > "$RTK_DRIFT/.anolisa-rtk-commit"
if bash "$RTK_SETUP" "$RTK_DRIFT" >"$TEMPORARY/rtk-drift.log" 2>&1; then
    printf 'ERROR: mismatched RTK revision marker was accepted\n' >&2
    exit 1
fi
grep -Fq "does not match pinned commit $PINNED_RTK_COMMIT" \
    "$TEMPORARY/rtk-drift.log"

OPENCLAW_ROOT="$REPO_ROOT/src/tokenless/adapters/tokenless/openclaw"
OPENCLAW_FIXTURE="$TEMPORARY/openclaw"
TOKENLESS_VERSION="$(python3 - "$REPO_ROOT/src/tokenless/Cargo.toml" <<'PY'
import sys
import tomllib
from pathlib import Path


manifest = tomllib.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
print(manifest["workspace"]["package"]["version"])
PY
)"
python3 - "$OPENCLAW_ROOT/package-lock.json" "$TOKENLESS_VERSION" <<'PY'
import json
import sys
from pathlib import Path


lockfile = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
expected = sys.argv[2]
versions = {
    lockfile.get("version"),
    lockfile.get("packages", {}).get("", {}).get("version"),
}
if versions != {expected}:
    raise SystemExit(f"OpenClaw lockfile version does not match Tokenless {expected}")
PY
install -d -m 0755 "$OPENCLAW_FIXTURE"
sed "s/@VERSION@/$TOKENLESS_VERSION/g" "$OPENCLAW_ROOT/package.json.in" \
    > "$OPENCLAW_FIXTURE/package.json"
cp "$OPENCLAW_ROOT/package-lock.json" "$OPENCLAW_FIXTURE/package-lock.json"
(
    cd "$OPENCLAW_FIXTURE"
    npm ci --legacy-peer-deps --ignore-scripts --no-audit --no-fund >/dev/null
)
sed -i 's/"typescript": "\^5.8.0"/"typescript": "0.0.1"/' \
    "$OPENCLAW_FIXTURE/package.json"
if (
    cd "$OPENCLAW_FIXTURE"
    npm ci --legacy-peer-deps --ignore-scripts --no-audit --no-fund \
        >"$TEMPORARY/npm-drift.log" 2>&1
); then
    printf 'ERROR: npm lockfile drift was accepted\n' >&2
    exit 1
fi

MARKER="$TEMPORARY/injected"
if TOKENLESS_SOURCE_WORKTREE="$TEMPORARY/version-worktree/tokenless" \
    "$ACTION_DIR/build.sh" \
        --source-repo "$REPO_ROOT" \
        --output-dir "$TEMPORARY/version-output" \
        --version "0.7.12\$(touch ${MARKER})" \
        --target-os linux \
        --target-arch x86_64 \
        --profile gnu2.17-x86_64 \
        --tag '' >"$TEMPORARY/version.log" 2>&1; then
    printf 'ERROR: malicious version input was accepted\n' >&2
    exit 1
fi
[ ! -e "$MARKER" ] || {
    printf 'ERROR: malicious version input executed a command\n' >&2
    exit 1
}

if TOKENLESS_SOURCE_WORKTREE="$TEMPORARY/tag-worktree/tokenless" \
    "$ACTION_DIR/build.sh" \
        --source-repo "$REPO_ROOT" \
        --output-dir "$TEMPORARY/tag-output" \
        --version 0.7.12 \
        --target-os linux \
        --target-arch x86_64 \
        --profile gnu2.17-x86_64 \
        --tag "tokenless/v0.7.12\$(touch ${MARKER})" \
        >"$TEMPORARY/tag.log" 2>&1; then
    printf 'ERROR: malicious tag input was accepted\n' >&2
    exit 1
fi
[ ! -e "$MARKER" ] || {
    printf 'ERROR: malicious tag input executed a command\n' >&2
    exit 1
}

for project in tokenless-fixture rtk-fixture; do
    install -d -m 0755 "$TEMPORARY/$project/src"
    printf 'fn main() {}\n' > "$TEMPORARY/$project/src/main.rs"
    sed "s/@NAME@/$project/" > "$TEMPORARY/$project/Cargo.toml" <<'EOF'
[workspace]

[package]
name = "@NAME@"
version = "1.0.0"
edition = "2021"
EOF
    cargo generate-lockfile --manifest-path "$TEMPORARY/$project/Cargo.toml"
done

printf 'Tokenless SBOM fixture\n' > "$TEMPORARY/payload.txt"
ARTIFACT_ROOT="$TEMPORARY/artifacts"
for target in \
    'linux x86_64 x86_64-unknown-linux-gnu' \
    'linux aarch64 aarch64-unknown-linux-gnu' \
    'macos aarch64 aarch64-apple-darwin'; do
    read -r target_os target_arch target_triple <<<"$target"
    artifact_dir="$ARTIFACT_ROOT/tokenless-prebuilt-1.0.0-$target_os-$target_arch"
    archive="$artifact_dir/tokenless-1.0.0-$target_os-$target_arch.tar.gz"
    install -d -m 0755 "$artifact_dir"
    tar -C "$TEMPORARY" -czf "$archive" payload.txt
    (
        cd "$artifact_dir"
        sha256sum "${archive##*/}" > "${archive##*/}.sha256"
    )
    python3 "$COMMON_DIR/generate-sbom.py" \
        --artifact "$archive" \
        --component tokenless \
        --version 1.0.0 \
        --os "$target_os" \
        --arch "$target_arch" \
        --target "$target_triple" \
        --project-dir "$TEMPORARY/tokenless-fixture" \
        --project-dir "$TEMPORARY/rtk-fixture" \
        --source-date-epoch 0 >/dev/null
    python3 "$COMMON_DIR/verify-artifacts.py" \
        --directory "$artifact_dir" \
        --component tokenless \
        --version 1.0.0 \
        --layout flat \
        --os "$target_os" \
        --arch "$target_arch"
done

python3 "$COMMON_DIR/verify-artifacts.py" \
    --directory "$ARTIFACT_ROOT" \
    --component tokenless \
    --version 1.0.0 \
    --layout actions

python3 - "$ARTIFACT_ROOT/tokenless-prebuilt-1.0.0-linux-x86_64/tokenless-1.0.0-linux-x86_64.tar.gz.cdx.json" <<'PY'
import json
import sys
from pathlib import Path


document = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
components = list(document["components"])
components.extend(document["metadata"]["component"].get("components", []))
names = {component["name"] for component in components}
expected = {"tokenless-fixture", "rtk-fixture"}
missing = expected - names
if missing:
    raise SystemExit(f"multi-project SBOM is missing components: {sorted(missing)}")
PY

MISSING_ROOT="$TEMPORARY/missing-artifacts"
cp -a "$ARTIFACT_ROOT" "$MISSING_ROOT"
rm "$MISSING_ROOT/tokenless-prebuilt-1.0.0-linux-x86_64/"*.sha256
if python3 "$COMMON_DIR/verify-artifacts.py" \
    --directory "$MISSING_ROOT" \
    --component tokenless \
    --version 1.0.0 \
    --layout actions >/dev/null 2>&1; then
    printf 'ERROR: incomplete Actions Artifact set was accepted\n' >&2
    exit 1
fi

EXTRA_ROOT="$TEMPORARY/extra-artifacts"
cp -a "$ARTIFACT_ROOT" "$EXTRA_ROOT"
touch "$EXTRA_ROOT/tokenless-prebuilt-1.0.0-linux-x86_64/unexpected"
if python3 "$COMMON_DIR/verify-artifacts.py" \
    --directory "$EXTRA_ROOT" \
    --component tokenless \
    --version 1.0.0 \
    --layout actions >/dev/null 2>&1; then
    printf 'ERROR: Actions Artifact set with an extra file was accepted\n' >&2
    exit 1
fi

printf 'Tokenless prebuilt action tests passed\n'
