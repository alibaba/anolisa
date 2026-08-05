# anolisa-tokenless

LLM token optimization toolkit — schema/response compression, command rewriting, and tool environment readiness.

## Install

```bash
npm install -g anolisa-tokenless
```

This automatically installs the correct prebuilt binary for your platform.

## Binaries

| Binary | Description |
|--------|-------------|
| `tokenless` | Main CLI — schema compression, response compression, TOON encoding, stats |
| `rtk` | Command rewriting engine (filters CLI output noise) |
| `toon` | TOON (Token-Oriented Object Notation) format encoder |

## Platform Support

| Platform | Architecture | Package |
|----------|-------------|---------|
| Linux (glibc) | x86_64 | `@anolisa/tokenless-linux-x64` |
| Linux (glibc) | aarch64 | `@anolisa/tokenless-linux-arm64` |
| macOS | x86_64 (Intel) | `@anolisa/tokenless-darwin-x64` |
| macOS | aarch64 (Apple Silicon) | `@anolisa/tokenless-darwin-arm64` |

The correct platform-specific binaries are automatically installed via `optionalDependencies`.

> **glibc only:** the Linux binaries target `*-unknown-linux-gnu` with a
> pinned minimum baseline of **GLIBC 2.17**, and the Linux platform packages
> declare `"libc": ["glibc"]`. musl-based distributions (e.g. Alpine) are not
> supported — build from source there instead.

## Framework Adapters

The root package bundles the Tokenless framework adapters (cosh, OpenClaw,
Hermes, qoder, claude-code, codex, qwencode). The adapter hooks are plain
bash/python scripts — OS and architecture independent — so they work on both
Linux and macOS.

On install, they are copied to the user-level data directory searched by the
hook dispatcher:

```
~/.local/share/anolisa/adapters/tokenless/
```

To register an adapter with your agent framework, run its install script,
e.g. for Claude Code:

```bash
bash ~/.local/share/anolisa/adapters/tokenless/claude-code/scripts/install.sh
```

## Usage

```bash
# Compress an API response
tokenless compress-response -f response.json

# Compress tool schemas
tokenless compress-schema -f tools.json

# Encode JSON to TOON format
tokenless compress-toon -f data.json

# Decode TOON back to JSON
tokenless decompress-toon -f data.toon

# Command rewriting (filters CLI output noise)
rtk ls -la
# Or use rewrite subcommand
rtk rewrite "ls -la"

# Check tool environment readiness
tokenless env-check --all
```

## Build from Source

Source builds are **Linux-only**. macOS users should install the prebuilt
binaries via `npm install -g anolisa-tokenless`; the macOS CLI binaries are
cross-compiled from Linux and published as npm platform packages.

If no prebuilt binary is available for your platform, or you want to build on
Linux from source:

```bash
git clone https://github.com/alibaba/anolisa.git
cd anolisa/src/tokenless
make build
make install
```

### Prerequisites

- **Linux** host (glibc-based distribution)
- **Rust** toolchain >= 1.91 (required by rtk v0.43.0)
- **just** — build runner for rtk setup
- **Git** — for rtk source download

## Packaging for npm

```bash
# Build for current platform
node scripts/package-npm.js

# Build for all supported targets
node scripts/package-npm.js --all

# Build for a specific target
node scripts/package-npm.js --target darwin-arm64
```

### Cross-Compilation (Linux → All Platforms)

On a Linux machine, you can cross-compile to **all 4 targets** (including macOS) using `cargo-zigbuild`.

#### 1. Install the toolchain

```bash
# Install cargo-zigbuild and zig
cargo install cargo-zigbuild
pip install ziglang   # or: apt install zig / brew install zig

# Add Rust targets
rustup target add aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin
```

#### 2. Prepare a macOS SDK (required for Apple targets)

Zig only provides the C toolchain — linking Apple targets additionally
requires macOS SDK headers and libraries. Download and extract an SDK, then
point `SDKROOT` at it (the packaging script fails fast if `SDKROOT` is not
set for a Linux → macOS build):

```bash
# Download an extracted macOS SDK (e.g. 13.3)
curl -LO https://github.com/joseluisq/macosx-sdks/releases/download/13.3/MacOSX13.3.sdk.tar.xz
sudo tar -xf MacOSX13.3.sdk.tar.xz -C /opt
export SDKROOT=/opt/MacOSX13.3.sdk
```

> Note: make sure your use of the macOS SDK complies with the
> [Xcode SDK license agreement](https://www.apple.com/legal/sla/docs/xcode.pdf).

An alternative that avoids manual SDK setup:

- **Official cargo-zigbuild Docker image** (ships with a macOS SDK preinstalled):

  ```bash
  docker run --rm -it -v $(pwd):/io -w /io messense/cargo-zigbuild \
    cargo zigbuild --release --target aarch64-apple-darwin
  ```

#### 3. Build all platforms

```bash
make npm-package-all
# or directly:
node npm/scripts/package-npm.js --all
```

The script auto-detects available cross-compilation tools in this order:

| Tool | Command | macOS from Linux? | Notes |
|------|---------|-------------------|-------|
| `cargo-zigbuild` | `cargo install cargo-zigbuild && pip install ziglang` | ✅ Yes | Recommended. Uses Zig's C toolchain as linker |
| `cross` | `cargo install cross` + Docker | ❌ No | Linux targets only from Linux |
| `cargo` (native) | Built-in | ❌ No | Host target only |

If no cross-compilation tool is available, the script will warn you and suggest installing `cargo-zigbuild`.

> Linux targets are always routed through `cargo-zigbuild` when it is
> available — even on a matching host — so the minimum GLIBC baseline is
> pinned to 2.17 instead of inherited from the build machine. The script
> verifies the resulting binaries with `readelf` and fails if any symbol
> requires a newer GLIBC.

### Publishing

```bash
make npm-publish
```

This packages all targets, then publishes the four platform packages first
and the root package last. The registry is pinned to
`https://registry.npmjs.org/` both in the generated manifests
(`publishConfig`) and on the publish command line, and already-published
versions are skipped so a partially failed run can be safely re-executed.

## License

Apache License 2.0
