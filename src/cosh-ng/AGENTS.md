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

### cosh-shell Testing Strategy

cosh-shell 的 PTY 集成测试较慢（每个 spawn 子进程）。开发时使用分层策略，避免跑全量：

```bash
# 开发时：只跑单元测试（0.1s）
cargo test --package cosh-shell --lib

# 验证逻辑：跑 logic target
cargo test --package cosh-shell --test logic

# 验证协议：跑 protocol target
cargo test --package cosh-shell --test protocol

# 验证单个集成测试（0.5-2s）
cargo test --package cosh-shell --test raw_cli <test_name> -- --exact

# 验证 shell host 改动（用并行加速）
cargo test --package cosh-shell --test shell_host -- --test-threads=4

# 阶段验收才跑全量（并行）
cargo test --package cosh-shell -- --test-threads=4
```

cosh-shell 测试布局必须遵循 `../../../specs/shell-test-organization/standard.md`：

- `src/` 只放 private 纯逻辑或轻量 component tests。
- public API 多模块逻辑测试进入 logic layer，目标 target 是 `logic`。
- adapter/control protocol 测试进入 protocol layer，目标 target 是 `protocol`。
- spawn `cosh-shell` binary、scripted raw shell、approval/question card、provider handoff 进入 `raw_cli` layer。
- PTY shell host、OSC、termios、foreground/native shell 行为进入 `shell_host` layer。
- 真实 provider、manual TTY、视觉/体验验证归入 `specs/shell-e2e-validation`，不混入默认 cargo test gate。

联合布局审计入口：

```bash
crates/cosh-shell/scripts/check-layout.sh
```

该脚本必须保持通过；新增或迁移代码不能增加新的 violation group。脚本中的 registered debt 只表示迁移债务被 inventory 追踪，不代表最终验收已完成。

测试改动 review 必须同时参考 `../../../specs/shell-test-organization/review.md`；代码组织改动 review 必须同时参考 `../../../specs/cosh-ng-code-organization/review.md`。AI reviewer 发现测试落位、命名、fixture/helper、heavy gate、root facade、public API、self-crate path 或 dependency direction 问题时，应按对应 review 文档给出纠错意见。

Prerequisites: Linux (or macOS for limited functionality), Rust 1.70+. pkg/svc commands need root/sudo. Checkpoint commands need a running ws-ckpt daemon.

## Architecture

5-crate workspace with strict dependency direction: `cosh-cli` / `cosh-tui` / `cosh-shell` → `cosh-platform` → `cosh-types`

- **cosh-types**: Pure types, zero side effects. Defines `CoshResponse<T>` envelope, `CoshError` (with error codes, recoverable flag, hint), and ws-ckpt IPC protocol types.
- **cosh-platform**: Platform abstraction layer. Distro detection from `/etc/os-release`, package manager routing (dnf/apt/zypper/brew), systemd service adapter, ws-ckpt daemon Unix socket IPC client.
- **cosh-cli**: CLI entry point (binary: `cosh`). 4 command domains: `pkg`, `svc`, `checkpoint`, `audit`. All output is JSON via `CoshResponse<T>`. Uses clap derive for argument parsing.
- **cosh-tui**: Interactive TUI (binary: `cosh-tui`). Uses ratatui + crossterm. Has slash commands, optional LLM chat, theme system.
- **cosh-shell**: AI-augmented interactive shell (binary: `cosh-shell`). PTY wrapper over bash/zsh with OSC marker-based command boundary detection, streaming AI analysis (Claude/Qwen adapters), inline card rendering, tool approval control protocol. See [`docs/cosh-shell-architecture.md`](docs/cosh-shell-architecture.md) for detailed architecture.

### cosh-shell Code Organization

cosh-shell 代码组织必须遵循 `../../../specs/cosh-ng-code-organization/standard.md` 和 `../../../specs/cosh-shell-joint-execution-plan.md`。本轮改造只允许动 `crates/cosh-shell/`；不要顺手修改 `cosh-types`、`cosh-platform`、`cosh-cli`、`cosh-tui`、lockfile、`.env` 或 workspace 外代码。

长期 owner 约定：

- UI owner 使用 `ui/`；`agent_render/` 只允许作为短期兼容 facade。
- Hook owner 使用 `hooks/`；`hook_engine/` 合并入 `hooks/`。
- Linux memory hook 收敛到 `hooks/linux_memory/`。
- `context_window` -> `evidence/context_window.rs`。
- `exit_classify` -> `command/exit_classify.rs`。
- `governance` -> `agent/governance.rs`。
- `interactive` -> `shell_host/line_interactive.rs`。
- `hook_types` 拆分到 `types/hooks.rs` 和 `hooks/model.rs`。

新增 cosh-shell 代码时：

- 不新增 root `crates/cosh-shell/src/*.rs` implementation 文件。
- 不新增未登记的 `lib.rs pub mod` 或 root re-export。
- `src/` production code 不新增 `cosh_shell::...` self-crate public path；使用 `crate::...` 或 owner module path。
- 不向超过 1000 行的 production 文件追加新功能；超过 700 行的 production 文件需要 owner note、拆分计划或 waiver。
- `hooks` 不直接拥有 agent 启动或 runtime state mutation；通过 runtime command/event 边界交接。

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
