#!/usr/bin/env bash
# Verify npm packaging consumes, validates, and preserves prebuilt binaries.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d /tmp/tokenless-npm-package-test.XXXXXX)"
PREBUILT="$ROOT/target/npm-prebuilt"
BACKUP="$TMP/npm-prebuilt.backup"
TEST_TOOLS="$TMP/tools"

cleanup() {
    rm -rf "$PREBUILT"
    if [[ -e "$BACKUP" ]]; then
        mv "$BACKUP" "$PREBUILT"
    fi
    rm -rf "$TMP"
}
trap cleanup EXIT

if [[ -e "$PREBUILT" ]]; then
    mv "$PREBUILT" "$BACKUP"
fi

mkdir -p "$TEST_TOOLS"
cat >"$TEST_TOOLS/readelf" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
input="${@: -1}"
grep -aoE 'GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?' "$input" || true
SH
chmod 0755 "$TEST_TOOLS/readelf"
export PATH="$TEST_TOOLS:$PATH"

make_binaries() {
    local target="$1"
    local destination="$PREBUILT/$target"

    mkdir -p "$destination"
    python3 - "$target" "$destination" <<'PY'
import pathlib
import struct
import sys

target, destination = sys.argv[1:]
root = pathlib.Path(destination)
if target.startswith("linux-"):
    machine = {"linux-x64": 62, "linux-arm64": 183}[target]
    header = bytearray(64)
    header[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<H", header, 16, 2)
    struct.pack_into("<H", header, 18, machine)
    struct.pack_into("<I", header, 20, 1)
    content = bytes(header) + b"\nGLIBC_2.17\n"
else:
    cpu = {"darwin-x64": 0x01000007, "darwin-arm64": 0x0100000C}[target]
    content = struct.pack("<IiiIIIII", 0xFEEDFACF, cpu, 0, 2, 0, 0, 0, 0)
for name in ("tokenless", "rtk", "toon"):
    (root / name).write_bytes(content + f"\n{name}-{target}\n".encode())
PY
    chmod 0755 "$destination/tokenless" "$destination/rtk" "$destination/toon"
}

for target in linux-x64 linux-arm64 darwin-x64 darwin-arm64; do
    make_binaries "$target"
done

cp "$PREBUILT/linux-x64/"* "$PREBUILT/linux-arm64/"
if node "$ROOT/npm/scripts/package-npm.js" --target linux-arm64 \
    >"$TMP/mismatch.out" 2>&1; then
    echo "ERROR: npm packer accepted mislabeled x86_64 binaries as arm64" >&2
    exit 1
fi
grep -Fq 'does not match linux-arm64' "$TMP/mismatch.out"
make_binaries linux-arm64

python3 - "$PREBUILT/linux-x64/tokenless" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
path.write_bytes(path.read_bytes().replace(b"GLIBC_2.17", b"GLIBC_2.18"))
PY
if ! node "$ROOT/npm/scripts/package-npm.js" --target linux-x64 \
    >"$TMP/glibc.out" 2>&1; then
    echo "ERROR: npm packer rejected a binary above the GLIBC 2.17 baseline" >&2
    exit 1
fi
grep -Fq 'requires GLIBC_2.18' "$TMP/glibc.out"
make_binaries linux-x64

check_selector() {
    local selector="$1"
    local expected="$2"
    node "$ROOT/npm/scripts/package-npm.js" --target "$selector" >"$TMP/selector.out"
    grep -Fq "Targets: $expected" "$TMP/selector.out"
}

check_selector x86_64 'linux-x64, darwin-x64'
check_selector arm64 'linux-arm64, darwin-arm64'
check_selector linux 'linux-x64, linux-arm64'
check_selector aarch64-apple-darwin 'darwin-arm64'

node "$ROOT/npm/scripts/package-npm.js" --all

for target in linux-x64 linux-arm64 darwin-x64 darwin-arm64; do
    package="$ROOT/npm/dist/tokenless-$target"
    test -f "$package/package.json"
    test -f "$package"/*.tgz
    for binary in tokenless rtk toon; do
        cmp "$PREBUILT/$target/$binary" "$package/bin/$binary"
        test "$(stat -c '%a' "$package/bin/$binary")" = 755
    done
done

node - "$ROOT/npm/dist/tokenless/package.json" <<'JS'
const fs = require('node:fs');
const manifest = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const expected = [
  '@anolisa/tokenless-linux-x64',
  '@anolisa/tokenless-linux-arm64',
  '@anolisa/tokenless-darwin-x64',
  '@anolisa/tokenless-darwin-arm64',
];
for (const name of expected) {
  if (manifest.optionalDependencies?.[name] !== manifest.version) {
    throw new Error(`missing optional dependency ${name}`);
  }
}
JS

if grep -Eq \
    'cargo-zigbuild|cargo zigbuild|cross build|rustup target|SDKROOT|detectBuilder' \
    "$ROOT/npm/scripts/package-npm.js"; then
    echo "ERROR: npm packer still contains cross-compilation logic" >&2
    exit 1
fi
echo "tokenless prebuilt npm package tests passed"
