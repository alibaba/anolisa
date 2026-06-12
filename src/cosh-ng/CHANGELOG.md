# Changelog

All notable changes to cosh-ng will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-05-16

Hardening + audit-subsystem release. Workspace versions bumped to `0.2.0` together with the release profile and lockfile commit.

### Added

- **`audit` subsystem** with PEP/PDP/log split: `cosh audit check` / `cosh audit log` for command-safety gating and per-session retrieval.
- **Workspace release profile** (`opt-level = 3`, `lto = true`, `strip = true`, `codegen-units = 1`), committed `Cargo.lock`, workspace-level dependency pinning, and native CA cert support.
- **Command timeouts, input validation, and panic-safe JSON output** across `cosh-cli` and `cosh-platform` so a panic still emits a `CoshResponse` envelope on stderr instead of an empty exit.
- **`forbid(unsafe_code)`** on `cosh-cli` / `cosh-platform`, plus `svc list --state` filter validation against an allow-list.
- **`pkg search` cross-references installed status** so results show which matches are already installed.
- **`ResponseMeta.warning`** field for non-fatal warnings; `audit` responses are explicitly marked as stub via this field.
- **LLM tool surface expansion** in cosh-tui: pkg / svc / checkpoint wrapper tools, plus `svc enable` / `svc disable --dry-run`.
- **Timeouts + exponential-backoff retries** on LLM and external command tools in cosh-tui; 60 s shell-tool timeout.

### Changed

- TUI `/help` aligned with the full command set; title bar version and markdown prefix stripping corrected.
- Clippy warnings resolved across the workspace; dead-code allowances dropped; test code aligned with production lint level.
- Build warnings eliminated and version detection improved across cosh-tui / cosh-platform.

### Fixed

- **Shell safety check tokenized** to close tab / newline / redirect / chain bypasses; substring matching on raw command strings replaced with whitespace (incl. `\t` / `\n` / `\r`) tokenization and metacharacter rejection (`;` `|` `&` `>` `<` `$` `` ` `` `(` `)` `{` `}`) — `is_safe_command` in `crates/cosh-tui/src/tools/shell.rs`.
- Forbidden tool calls are now blocked even under Yolo approval mode.
- `cosh-cli` wrapper tool output is bounded so a chatty subcommand cannot blow the LLM context window.
- Tool-call IDs synthesized via a process-wide counter to guarantee uniqueness across the agentic loop.
- `settings.json` and session files written atomically with `0600` permissions.
- Runtime bounds enforced for the agentic loop, history, config, and tool messages; scrollback bounded with UTF-8-safe truncation.
- Panic hook installed in the TUI; history navigation recovered after panic.
- ws-ckpt IPC response size bounded to 64 MiB.
- Nonexistent systemd services detected via `LoadState=not-found` instead of misclassifying them as "inactive".

### Security

- Audit-stub `recoverable` / `hint` semantics surface clearly to agents via the standard `CoshError` envelope.
- Atomic-rename + `0600` perms on credential-bearing files.

## [0.1.0] - 2026-05-10

Initial public-shaped release after renaming the workspace from `agos-core` to `cosh-ng` and adding the interactive TUI crate.

### Added

- **4-crate workspace**: `cosh-types`, `cosh-platform`, `cosh-cli`, `cosh-tui` with strict dependency direction `cosh-cli` / `cosh-tui` → `cosh-platform` → `cosh-types`.
- **`cosh` CLI binary** with dual-mode dispatch: `cosh` (no args) execs into `cosh-tui`, `cosh <subsystem> <action>` returns structured JSON.
- **Cross-distro `pkg` subsystem**: `install` / `remove` / `search` / `list` routed across `dnf` / `apt-get` (`apt-cache` for search) / `zypper` based on `Distro::detect()` reading `/etc/os-release`.
- **`svc` subsystem** over `systemctl`: `status` / `start` / `stop` / `restart` / `enable` / `disable` / `list`, with uptime and corrected column mapping in `list`.
- **`checkpoint` subsystem** talking to the `ws-ckpt` daemon over Unix-socket IPC; bincode wire format with 4-byte LE length prefix and explicit protocol versioning + error handling. Commands: `init` / `create` / `list` / `restore` / `recover` / `delete` / `diff` / `cleanup` / `status`.
- **`cosh-tui`** interactive TUI on `ratatui` + `crossterm`: slash-command system with auto-complete, session management, theming, custom border set, echo-on-submit.
- **Agentic loop with cosh-cli wrapper tools** in cosh-tui, bringing pkg / svc / checkpoint tooling to the LLM (initially shipped as `cosh-tui v0.4.0`).
- **LLM chat integration** with config-driven providers and UI surfacing.
- **Unified `settings.json` V2 config** consolidating prior scattered config files.
- **AES-256-GCM decryption** for encrypted credentials.
- **macOS detection + Homebrew backend** in `cosh-platform`, with unit tests.
- **Unified JSON envelope** `CoshResponse<T>` with `ok` / `data` / `error` / `meta`, classified `CoshError` carrying `recoverable` and `hint` for agent retry decisions.
- **Integration tests** for `pkg` and `checkpoint` CLI commands.

### Changed

- Workspace renamed from `agos-core` (with `agos-types` / `agos-platform` / `agos-cli`) to `cosh-ng` (with `cosh-*` crates); `agos-cli` and `agos-platform` removed in the same commit.
- `cosh-tui` checkpoint tooling adapted to the new daemon protocol.

### Fixed

- `cosh-cli` stdout validated as JSON before forwarding to the LLM, preventing parser confusion on malformed bytes.

## [pre-0.1.0] - 2026-05-03 → 2026-05-08

Pre-rename `agos-core` foundation.

### Added

- Initial 2-crate workspace `agos-types` + `agos-platform`.
- `agos-cli` cross-distro CLI prototype with `pkg`, `svc`, `checkpoint`, `audit` command shapes.
- MVP v2 CLI Gateway architecture document and bilingual (English / Chinese) usage guide.
