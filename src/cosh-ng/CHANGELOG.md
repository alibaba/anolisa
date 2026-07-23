# Changelog

All notable changes to the cosh-ng project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.13.0] — Unreleased

### Added
- Interactive session recovery: discover, select, resume, continue, inspect, and clear provider sessions from the shell via `/session`, `/resume`, and the `--resume` launch option (#1546)
- Workspace-scoped session persistence with schema versioning, legacy-session migration, and a provider-free `--session-control` management protocol in cosh-core (#1546)

## [0.12.0] — 2026-07-12

### Added
- Move authentication ownership into cosh-core with isolated config layers (f028ad90)

### Changed
- Consolidate logging under a unified runtime module (db86b3dd)
- Migrate component docs into user-guide/developer-guide and add cosh-ng docs (317d3f26, adf63ac2)
- Rename `*_CN.md` docs to `*_zh.md` and fix cross-references (82f8dab4)

### Fixed
- Honor svc dry-run across platform and cli (4c593050)
- Preserve manual aliyun fallback and legacy STS auth (924dd76b, ee1dd179)
- Protect auth provider edits; prioritize aliyun auth option (f0c97efa, 904655fb)
- Bound host-executed shell preview (a6da7301)
- Route noninteractive cosh launcher calls; support raw command passthrough (ecb56739, 1490eb3c)
- Bind startup prompt and agent request context (14d6336f, 25d9d28f)
- Own prompt boundary in shell (#1310)
- Avoid UTF-8 split in loop detection (ef7f5147)
- Remove provider-visible skill hints; guide diagnostic skill use (a6024873, 7f695178)
- Drop redundant format borrow; satisfy clippy diagnostics (5e14a686, 063217f1)
- Stabilize CI, raw-cli, PTY, and service tests (d573796d, 3563a5ab, 0fb34ea6, fc28da5f, ad138d45, 65421d25, 5b17b892, 3e43ba6e, 707bc3c0)

## [0.11.0] — 2026-06-28

### Added
- Aliyun authentication provider with ECS auto-detection, STS credentials, and QR code flow
- SysOM Aliyun provider with ACS3 signing for LLM API access
- Per-turn SLS JSONL logging for observability
- SysOM request source identification headers
- Structured tracing logging system across all crates
- Sandbox bypass approval flow on PostToolUseFailure hook events
- Startup health scan for environment diagnostics
- Extension/hook/skill enable/disable commands (`/extensions`, `/hooks`, `/skills`)
- Unified component state management module
- Dedicated HOOK approval panel with simplified UI
- UserPromptSubmit hook with Ask approval enforcement
- Shell evidence read admission control in cosh-core
- Tool activity rendering in cosh-shell
- `cosh-switch` hint in startup banner for toggling between cosh-ng and copilot-shell

### Changed
- **BREAKING**: Rename CLI binary from `cosh` to `cosh-cli`; remove dispatch_core
- RPM spec: install cosh-cli binary, `/usr/bin/cosh` launcher, cosh-switch script, `Conflicts: copilot-shell`
- Replace eprintln with structured tracing macros
- Unify hook decision aggregation with fold_decision
- Update workspace repository URL to github.com/alibaba/anolisa

### Fixed
- Auth ECS flow and panel overlap on phase transitions
- Auth QR code rendered without ANSI escape codes
- Use SysomProvider for aliyun after auth success
- Align hook input fields and AfterModel/wrap_tool_response with copilot-shell protocol
- tool_result dedup guard and visibility
- Restore prompt before shell handoff
- Suppress duplicate evidence reads
- Reduce failed-command auto analysis noise
- Skill existence check and harden approval matching
- Resolve clippy warnings across cosh-core and cosh-shell

## [0.10.0] — 2026-06-23

### Added
- Shell evidence protocol for capturing and replaying command execution context
- Shell evidence control tool in cosh-core for evidence lifecycle management
- Hook protocol aligned with copilot-shell for zero-change extension support
- Per-hook decision propagation through notification protocol
- Hook warnings rendered with per-hook decision color-coding in cosh-shell

### Changed
- Share agent error display text across cosh-shell modules

### Tests
- Cover shell evidence raw CLI flows

## [0.9.0] — 2026-06-22

### Added
- Hook notifications integrated into approval panel with ⚠ warning display
- Hook ask decisions enforce user approval even in Trust/Auto modes
- Extended hook system with tool_use_id association and new event types
- Registry protocol for /extensions /skills /hooks slash commands

### Fixed
- Show Registry group in /help output

### Changed
- Remove dead skill management code

## [0.8.0] — 2026-06-18

### Changed
- Rename `cosh-tui` crate and binary to `cosh-core` across the entire workspace
- Update adapter system: `CoshTuiAdapter` → `CoshCoreAdapter`, `AdapterKind::CoshTui` → `CoshCore`
- Update environment variable `COSH_TUI_PATH` → `COSH_CORE_PATH`
- Update RPM spec, documentation, and all test fixtures

### Fixed
- Neutralize agent status text in streaming cards
- Align streaming card widths in cosh-shell

## [0.7.0] — 2026-06-17

### Added
- Extension discovery and loading module with `cosh-extension.json` manifest support
- Extension hooks integrated into startup lifecycle
- Skill module with multi-level loading (built-in, user, project) and hot-reload
- SkillManager integrated into tool registry and startup
- Available skills injected into system prompt for LLM discovery

### Fixed
- Infinite loop in `expand_env_vars` when environment variable is undefined
- Warn on unsupported extension hook events (`PostToolUseFailure`, `BeforeModel`, `AfterModel`) instead of silently discarding
- Align extension hooks format with copilot-shell nested group structure
- Remove unused `args` parameter from skill tool schema to avoid misleading LLM
- Show question free text answers in cosh-shell
- Harden foreground shell handoffs
- Share copilot shell config path and keep legacy config fallback

### Changed
- Normalize cosh-shell config keys
- Standardize cosh-shell code and test organization
- Move user state under copilot shell scope

## [0.6.0] — 2026-06-16

### Added
- P0 hook system with 5 lifecycle events (`on_session_start`, `on_turn_start`, `on_turn_end`, `on_tool_call`, `on_session_end`) in cosh-tui
- Shell approval classification and hook origin tracking in cosh-shell
- Migrate current cosh shell into monorepo workspace

### Fixed
- Address approval review findings in cosh-shell
- Harden shell evidence continuation to prevent dropped context
- Normalize tool call streaming protocol in cosh-tui
- Fix passthrough for subcommands in cosh-shell

## [0.5.0] — 2026-06-15

### Added
- CoshTuiAdapter persistent process mode (spawn once, reuse across agent runs, auto-restart on death)
- `ask_user` round-trip through control protocol (agent can ask inline questions routed to TUI)

### Changed
- Split cosh-tui main into cli/headless/interactive modules
- Rename binary from cosh-tui-core back to cosh-tui

## [0.4.1] — 2026-06-15

### Added
- settings.json → config.toml auto-migration with AES-256-GCM encrypted API key decryption
- JSONL protocol and tool approval integration tests

### Fixed
- Prepend precmd in PROMPT_COMMAND to capture real exit code (Alibaba Cloud Linux /etc/bashrc issue)

## [0.4.0] — 2026-06-15

### Added
- JSONL wire protocol (InputMessage / OutputMessage) for cosh-shell ↔ cosh-tui communication
- Provider abstraction with OpenAI-compatible streaming (DashScope, OpenAI, DeepSeek, Generic profiles)
- Tool execution framework with 7 built-in tools (shell, read_file, write_file, edit, grep, todo, skill) and approval control
- Context window management, message truncation, loop detection, conversation compression
- Lifecycle hooks framework
- CoshCore agent loop engine
- TOML-based multi-provider config with environment variable expansion

### Changed
- **BREAKING**: Binary interface from ratatui interactive TUI to JSONL stdin/stdout backend
- **BREAKING**: Config format from settings.json to config.toml
- Rewrite session store with single-file JSON persistence

### Removed
- Legacy ratatui-based TUI code (app, commands, llm, logger, theme, tools, ui modules)

## [0.3.0] — 2026-06-15

### Added
- **cosh-shell crate** — PTY-based AI-augmented shell host with OSC marker protocol
- Claude, Qwen, Fake AI adapters with streaming support
- Inline rendering engine (approval, question, recommendation, activity panels)
- Governance layer with approval modes
- Terminal recovery via signal handlers (SIGTERM/SIGHUP/SIGQUIT) and panic hook
- Exit code classification with 8 categories (Smart/Auto/Manual analysis modes)
- Tool display engine with per-tool-type parsing and ANSI color categories
- Hook engine with built-in hooks (FailedCommandHook, TestFailureHook) and skill routing
- External hook loading from ~/.config/cosh/hooks/ with subprocess execution
- Native shell compatibility (rcfile loading, PS1, history, login shell detection)
- Context window with sliding window (max commands, max age, token budget)
- Prompt intent optimization (do → Bash tool, know → prose)
- Natural language intercept with visual feedback
- InputClassifier conservative mode for native mode
- Analysis throttle (30s cooldown, max 3 consecutive)
- Consultation card rendering with keyboard capture
- Control protocol for tool approval round-trips
- Startup banner with gradient ASCII art logo
- `/mode` and `/hooks` slash commands
- Architecture documentation

### Fixed
- Native mode input rendering with powerlevel10k dual-line prompts
- Slash/NL intercept via buffered-then-judge strategy in native mode
- Zsh preexec intercept for command_not_found
- CandidateRedraw line clearing for CJK input and backspace
- Suppress cosh-osc$ prompt leak in native mode
- Tool display label matching in bash tool executor
- Wide character placeholder cell handling in buffer extraction

### Changed
- Unified workspace version (0.3.0) for all crates (cosh-types, cosh-platform, cosh-cli, cosh-shell, cosh-tui)

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
