# anolisa-tokenless

LLM token optimization toolkit — schema/response compression, command rewriting, and tool environment readiness.

> **Release status:** These npm packages are private and are not currently
> published to the public registry. This document describes the intended
> package layout and post-publication workflow; its platform rows are build
> targets, not a list of packages that users can install today.

## Install after publication

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

The npm packer reads prebuilt native executables from this fixed layout:

```text
target/npm-prebuilt/
├── linux-x64/{tokenless,rtk,toon}
├── linux-arm64/{tokenless,rtk,toon}
├── darwin-x64/{tokenless,rtk,toon}
└── darwin-arm64/{tokenless,rtk,toon}
```

Validating Linux packages requires GNU `readelf` from binutils. The packer
rejects binaries that require GLIBC symbols newer than the supported 2.17
baseline.

Package one target:

```bash
node npm/scripts/package-npm.js --target linux-arm64
```

For the current host, the Make entry point can build the native executables
before invoking the same packer:

```bash
make npm-package
```

Package the complete matrix:

```bash
node npm/scripts/package-npm.js --all
# or
make npm-package-all
```

Before copying anything, the packer checks that all three files exist and that
their ELF or Mach-O architecture matches the selected npm target; it never
executes cross-target binaries. Prebuilt Linux inputs must retain the documented
GLIBC 2.17 compatibility baseline.

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
