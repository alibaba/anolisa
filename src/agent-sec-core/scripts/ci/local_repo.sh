#!/usr/bin/env bash
# Publish a locally built raw archive as a file:// ANOLISA raw repository.
#
# The published raw repository is built from release tags, so an E2E suite in
# the working tree exercises code that can be several commits — or releases —
# older than the tests themselves. Passing the URL printed by this script to
# `anolisa install --backend raw --repo <url>` installs the archive built from
# the current tree, which removes that drift from the verification.
set -euo pipefail

die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# scripts/ci/ -> component root
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUILD_DIR="${BUILD_DIR:-$ROOT/target}"
OUTPUT_DIR="${OUTPUT_DIR:-$BUILD_DIR/raw}"
REPO_DIR="${REPO_DIR:-$BUILD_DIR/raw-repo}"
CONTRACT="${RAW_CONTRACT:-$ROOT/.anolisa/component.toml}"
TARGET_OS="${TARGET_OS:-linux}"
TARGET_ARCH="${TARGET_ARCH:-$(uname -m)}"
CHANNEL="${RAW_REPO_CHANNEL:-stable}"
PUBLISHER="${RAW_REPO_PUBLISHER:-sec-core-local-build}"
MARKER_NAME=".anolisa-local-raw-repo"
MARKER_CONTENT="anolisa-local-raw-repo-v1"

command -v python3 >/dev/null 2>&1 || die "python3 is required to read the raw contract"
[ -f "$CONTRACT" ] || die "raw contract not found: $CONTRACT"

# Identity, version, and install modes all come from the same contract the
# archive embeds, and the row carries that contract's digest — so a stale
# archive in OUTPUT_DIR is rejected at install time instead of being
# published under this tree's version.
IFS=$'\t' read -r COMPONENT VERSION INSTALL_MODES <<<"$(
    python3 - "$CONTRACT" <<'PY'
import sys
import tomllib

with open(sys.argv[1], "rb") as stream:
    component = tomllib.load(stream)["component"]
modes = component.get("layout", {}).get("modes", [])
if not modes:
    raise SystemExit("ERROR: contract declares no [component.layout] modes")
print("\t".join([component["name"], component["version"], ",".join(modes)]))
PY
)"
[ -n "$COMPONENT" ] && [ -n "$VERSION" ] && [ -n "$INSTALL_MODES" ] || \
    die "could not read component identity from $CONTRACT"

ARTIFACT="$OUTPUT_DIR/${COMPONENT}-${VERSION}-${TARGET_OS}-${TARGET_ARCH}.tar.gz"
[ -f "$ARTIFACT" ] || \
    die "raw archive not found: $ARTIFACT (run 'make package-raw' first)"

# Never replace an unrelated directory. A directory becomes managed only when
# this script creates its marker; an existing empty directory is safe to adopt.
[ -n "$REPO_DIR" ] && [ "$REPO_DIR" != "/" ] || \
    die "REPO_DIR must be a non-empty, non-root path"
case "$ARTIFACT" in
    "$REPO_DIR"/*) die "REPO_DIR must not contain the built archive: $ARTIFACT" ;;
esac
MARKER="$REPO_DIR/$MARKER_NAME"
if [ -e "$REPO_DIR" ] || [ -L "$REPO_DIR" ]; then
    [ -d "$REPO_DIR" ] && [ ! -L "$REPO_DIR" ] || \
        die "REPO_DIR must be a directory, not a file or symlink: $REPO_DIR"
    if [ -e "$MARKER" ] || [ -L "$MARKER" ]; then
        [ -f "$MARKER" ] && [ ! -L "$MARKER" ] && \
            [ "$(cat "$MARKER")" = "$MARKER_CONTENT" ] || \
            die "REPO_DIR has an invalid ownership marker: $MARKER"
    elif [ -n "$(find "$REPO_DIR" -mindepth 1 -print -quit)" ]; then
        die "refusing to replace non-empty unmanaged REPO_DIR: $REPO_DIR"
    fi
fi

# A stale v1/ would let install resolve an archive this run did not build,
# which is the very drift this repository exists to remove.
rm -rf "$REPO_DIR"
V1="$REPO_DIR/v1"
install -d -m 0755 "$V1"
printf '%s\n' "$MARKER_CONTENT" > "$MARKER"
chmod 0644 "$MARKER"
ARTIFACT_NAME="$(basename "$ARTIFACT")"
# The archive is a few hundred MB, so prefer a hard link and only fall back to
# a copy when the output and repository directories are on different volumes.
ln -f "$ARTIFACT" "$V1/$ARTIFACT_NAME" 2>/dev/null || \
    install -p -m 0644 "$ARTIFACT" "$V1/$ARTIFACT_NAME"
chmod 0644 "$V1/$ARTIFACT_NAME"

# sec-core is published in `index-v2.toml` only, because its contract relies on
# generation-2 semantics (`render = "anolisa-paths-v1"`) that pre-0.2.17
# clients cannot represent. Mirroring that here keeps the local repository
# resolvable by exactly the clients that can install the component.
INDEX="$V1/index-v2.toml"
TEMP_INDEX="$V1/.index-v2.toml.tmp.$$"
cleanup() {
    rm -f "$TEMP_INDEX"
}
trap cleanup EXIT

python3 - "$V1/$ARTIFACT_NAME" "$ARTIFACT_NAME" "$COMPONENT" "$VERSION" \
    "$TARGET_OS" "$TARGET_ARCH" "$INSTALL_MODES" "$CHANNEL" "$PUBLISHER" \
    "$CONTRACT" > "$TEMP_INDEX" <<'PY'
import hashlib
import pathlib
import sys

(
    artifact_path,
    artifact_name,
    component,
    version,
    target_os,
    target_arch,
    install_modes,
    channel,
    publisher,
    contract_path,
) = sys.argv[1:11]

payload = pathlib.Path(artifact_path).read_bytes()
# Packaging copies the contract into the archive byte for byte, so the digest
# of the file read here is the digest install recomputes from the archive.
contract_digest = hashlib.sha256(pathlib.Path(contract_path).read_bytes()).hexdigest()
modes = ", ".join(f'"{mode}"' for mode in install_modes.split(","))
print(
    "\n".join(
        [
            "# Generated by scripts/ci/local_repo.sh — do not publish.",
            "schema_version = 2",
            f'channel = "{channel}"',
            f'publisher = "{publisher}"',
            "",
            "[[entries]]",
            f'component = "{component}"',
            f'version = "{version}"',
            f'channel = "{channel}"',
            'artifact_type = "tar_gz"',
            'backend = "raw"',
            f'url = "{artifact_name}"',
            f'os = "{target_os}"',
            f'arch = "{target_arch}"',
            f"install_modes = [{modes}]",
            f'sha256 = "{hashlib.sha256(payload).hexdigest()}"',
            f'manifest_digest = "sha256:{contract_digest}"',
            f"size = {len(payload)}",
        ]
    )
)
PY

chmod 0644 "$TEMP_INDEX"
mv -f "$TEMP_INDEX" "$INDEX"

printf 'Published %s %s (%s/%s) to local raw repository\n' \
    "$COMPONENT" "$VERSION" "$TARGET_OS" "$TARGET_ARCH" >&2
printf 'file://%s\n' "$V1"
