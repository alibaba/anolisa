# ANOLISA CLI Build Guide

> 中文版: [BUILD_cn.md](BUILD_cn.md)

## Prerequisites

- Rust >= 1.88 (project uses edition 2024)
- Working directory: `src/anolisa/` (Cargo workspace root)

```bash
cd src/anolisa
```

---

## Local Development

```bash
# Compile only
cargo build -p anolisa-cli

# Compile and run
cargo run -p anolisa-cli -- env
cargo run -p anolisa-cli -- list
cargo run -p anolisa-cli -- enable agent-observability --dry-run

# Run tests
cargo test -p anolisa-core
cargo test --workspace
```

Output: `target/debug/anolisa`

---

## Production Build

```bash
cargo build --release -p anolisa-cli
```

Output structure:

```
target/release/anolisa          # main binary (symbol table retained, DWARF stripped)
target/release/anolisa.dwp      # split DWARF debug info (Linux)
target/release/anolisa.dSYM/    # debug info on macOS
```

Ship only the main binary; archive `.dwp` / `.dSYM` for coredump analysis:

```bash
# Place .dwp next to the binary, GDB discovers it automatically
gdb ./anolisa core.12345

# Or specify explicitly
gdb -s anolisa.dwp ./anolisa core.12345
```

---

## Cross-compilation (Linux x86_64 target)

```bash
# Add target
rustup target add x86_64-unknown-linux-gnu

# Cross-compile (requires matching linker, e.g. x86_64-linux-gnu-gcc)
cargo build --release -p anolisa-cli --target x86_64-unknown-linux-gnu
```

Output: `target/x86_64-unknown-linux-gnu/release/anolisa`

---

## Quick Reference

| Scenario | Command | Output |
|----------|---------|--------|
| Quick run | `cargo run -p anolisa-cli -- <subcmd>` | — |
| Debug build | `cargo build -p anolisa-cli` | `target/debug/anolisa` |
| Release build | `cargo build --release -p anolisa-cli` | `target/release/anolisa` |
