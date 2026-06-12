# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

cosh-ng (Computable Operating System Harness) is a deterministic Agent-OS interface. It provides a unified `cosh` binary with dual-mode behavior:
- **CLI mode** (`cosh <subsystem> <action>`): structured JSON output for AI Agents
- **Interactive mode** (`cosh` with no args): launches `cosh-tui` via exec

## Build & Test Commands

```bash
cargo build --workspace          # Build all crates
cargo test --workspace           # Run all tests (unit + integration)
cargo test --package cosh-cli --test cli_integration   # Integration tests only
cargo test --package cosh-platform   # Platform crate unit tests only
cargo test --package cosh-types      # Types crate unit tests only
```

Prerequisites: Linux (or macOS for limited functionality), Rust 1.70+. pkg/svc commands need root/sudo. Checkpoint commands need a running ws-ckpt daemon.

## Architecture

4-crate workspace with strict dependency direction: `cosh-cli` / `cosh-tui` → `cosh-platform` → `cosh-types`

- **cosh-types**: Pure types, zero side effects. Defines `CoshResponse<T>` envelope, `CoshError` (with error codes, recoverable flag, hint), and ws-ckpt IPC protocol types.
- **cosh-platform**: Platform abstraction layer. Distro detection from `/etc/os-release`, package manager routing (dnf/apt/zypper/brew), systemd service adapter, ws-ckpt daemon Unix socket IPC client.
- **cosh-cli**: CLI entry point (binary: `cosh`). 4 command domains: `pkg`, `svc`, `checkpoint`, `audit`. All output is JSON via `CoshResponse<T>`. Uses clap derive for argument parsing.
- **cosh-tui**: Interactive TUI (binary: `cosh-tui`). Uses ratatui + crossterm. Has slash commands, optional LLM chat, theme system.

## Key Design Constraints

- **ws-ckpt IPC wire format**: Uses bincode with 4-byte LE length prefix framing. Enum variant order in `WsCkptRequest`/`WsCkptResponse`/`WsCkptErrorCode` is the binary wire contract — **never reorder variants** without coordinating with the ws-ckpt daemon.
- **Unified JSON envelope**: Every CLI command returns `CoshResponse<T>` with `ok`, `data`/`error`, and `meta` fields. Exit code 0 = success, 1 = failure.
- **Cross-distro routing**: `Distro::detect()` reads `/etc/os-release` and routes to the correct package manager. Adding a new distro means adding a variant to the `Distro` enum in `cosh-platform/src/detect.rs` and updating the `pkg_manager()` method.
- **CLI helpers**: `print_success()`, `print_failure()`, `build_meta()` in `cosh-cli/src/main.rs` handle all JSON serialization and exit codes — command modules return `i32` exit codes.

## Security Heuristics

When writing safety gates that auto-approve commands, don't pattern-match substrings of the *raw* command — shell metas don't need spaces, and Tab/newline are word separators. Tokenize first (split on whitespace including `\t`/`\n`/`\r`), reject metacharacters anywhere (`;` `|` `&` `>` `<` `$` `` ` `` `(` `)` `{` `}`), then dispatch on tokens. When in doubt, fall through to user approval rather than auto-allow. New regression tests must cover Tab-separated, newline-separated, and unspaced-meta variants. Reference: `crates/cosh-tui/src/tools/shell.rs::is_safe_command`.

## Debugging Guidelines

- **No host mutation outside isolated environments**: Unless explicitly running inside a container, VM, or other isolated environment, never execute operations that modify host system state (installing/removing packages, changing system config, managing systemd services, etc.).
- **Require a rollback plan before execution**: Before performing any debugging operation with side effects, explicitly list the steps and their corresponding rollback steps. Every operation must be reversible.
- **Roll back all side effects after debugging**: Any system changes produced during debugging (temp files, env vars, service state changes, etc.) must be fully reverted to the original state once debugging is complete.
- **Prefer `--dry-run`**: cosh pkg/svc commands support `--dry-run` — always use it first to verify behavior without actual execution.

## Adding a New CLI Command Domain

1. Create `crates/cosh-cli/src/cmd/<domain>.rs` with a `<Domain>Commands` enum (clap Subcommand) and a `pub fn run(...)` returning `i32`
2. Add the domain to the `Commands` enum in `cosh-cli/src/main.rs`
3. Add return types to `cosh-types/src/`
4. Add platform logic to `cosh-platform/src/`
5. Add integration tests in `crates/cosh-cli/tests/cli_integration.rs`

## Production-Readiness Checklist

Don't trust development reports — verify before merging:

- `cargo test --workspace` — count must match the report.
- `cargo clippy --workspace --all-targets` — `--all-targets` is non-negotiable; the default omits test code, where most lint debt accumulates. "0 warnings" claims without `--all-targets` are misleading.
- `cargo build --workspace --release` — release profile catches optimization-only issues.
- For every "hardened against X" claim, write a PoC that *would have* triggered X and verify it now fails closed. Substring-based safety lists in particular need adversarial review.

## Commit Message Conventions

Strict [Conventional Commits](https://www.conventionalcommits.org/):

- `type(scope): subject` — types limited to `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`, `build`, `ci`, `chore`. **Do not use `harden:` / `cleanup:`** — they aren't standard. Map them: closing a known vulnerability → `fix:`; adding a new defensive mechanism → `feat:`; lint/dead-code cleanup → `chore:`.
- `scope` is the crate name (`cli`, `platform`, `tui`, `types`); use `cli,platform` for multi-crate changes.
- Subject in imperative mood, ≤ 72 chars, no trailing period. Body explains *why*, not *what*.

## Git History Hygiene

When consolidating many commits via `rebase -i`:

- **`-X theirs` silently drops content** in reorder+squash scenarios. When commit A and a later commit B both touch overlapping regions, `theirs` may keep only one side. Prefer letting conflicts pause the rebase, or verify with `git diff <new> <backup> --stat` afterward.
- When restoring lost content via `edit` + `git commit --amend`, target the **last commit that touches the file** in the new ordering, not the most thematically relevant commit. Earlier amends get re-overwritten by subsequent cherry-picks.
- Fold matching test commits into their parent feat/fix so reviewers see code + tests as one unit.
