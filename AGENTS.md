# AGENTS.md

This file provides context for AI coding assistants (Qoder, Claude, etc.) working in this repository.

## 1. Project Overview

**ANOLISA** is a monorepo for an Agentic OS — a server-side operating layer designed for AI agent workloads.

| Component | Path | Tech | Platform |
|-----------|------|------|----------|
| **copilot-shell** (`cosh`) | `src/copilot-shell/` | TypeScript / Node.js | All |
| **agent-sec-core** | `src/agent-sec-core/` | Rust + Python | Linux only |
| **agentsight** | `src/agentsight/` | Rust (eBPF) | Linux only |
| **tokenless** | `src/tokenless/` | Rust | Linux only |
| **agent-memory** (`memory`) | `src/agent-memory/` | Rust | Linux only |
| **os-skills** | `src/os-skills/` | Python / Shell | All |
| **anolisa** | `src/anolisa/` | Rust | Linux + macOS (arm64) |
| **SkillFS** (`skillfs`) | `src/skillfs/` | Rust / FUSE | Linux only |
| **ws-ckpt** | `src/ws-ckpt/` | Rust + TypeScript | Linux only |

> `agent-sec-core`, `agentsight`, `tokenless`, `agent-memory`, and `skillfs` require Linux. Do **not** attempt to build them on macOS or Windows.

## 2. Development Commands

```bash
# Unified build (recommended — handles deps, build, and system install)
./scripts/build-all.sh                                        # all default components
./scripts/build-all.sh --no-install                           # build only, skip install
./scripts/build-all.sh --ignore-deps                          # skip dep installation
./scripts/build-all.sh --component cosh --component sec-core  # selected components

# Unified test runner
./tests/run-all-tests.sh
./tests/run-all-tests.sh --filter shell   # copilot-shell only
./tests/run-all-tests.sh --filter sec     # agent-sec-core only
./tests/run-all-tests.sh --filter sight   # agentsight only

# copilot-shell (per-component)
cd src/copilot-shell
make deps      # npm install + husky hooks (use make deps-ci in CI)
make build
make lint
make test

# agent-sec-core (Linux only, per-component)
cd src/agent-sec-core
make build-sandbox
pytest tests/integration-test/ tests/unit-test/ -v

# agentsight (Linux only, optional, per-component)
cd src/agentsight
make build
cargo test

# os-skills
cd src/os-skills   # Skill definitions are static assets, no compilation needed

# tokenless (per-component)
cd src/tokenless
cargo build --release
cargo test

# agent-memory (Linux only, per-component)
cd src/agent-memory
make build       # cargo build --release --locked
make test        # cargo test --locked
make smoke       # end-to-end MCP stdio smoke test

# anolisa (per-component)
cd src/anolisa
cargo fmt --all --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked

# ws-ckpt (Linux only, per-component)
cd src/ws-ckpt
make build       # cargo build --release + openclaw plugin
make test        # cargo test --workspace

# SkillFS (Linux only, per-component)
cd src/skillfs
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
scripts/test.sh   # FUSE smoke test; skips itself if fuse3 or /dev/fuse is unavailable
```

## 3. Rust Common Conventions

> Applies to all Rust components: `anolisa`, `agentsight`, `tokenless`, `agent-memory`, `skillfs`.

### 3.1 Comment Guidelines

Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) and the official style guide. Write comments that help readers understand intent faster — not comments that paraphrase code.

**Comment types and placement:**

- `//!` **module-level docs**: at the top of a file/module — one or two sentences describing what the module does and when to use it.
- `///` **doc comments**: required on all public (`pub`) items — structs, enums, traits, functions, methods, significant fields, and variants. These appear in `cargo doc`.
- `//` **inline comments**: only where the implementation needs to explain *why* something is done a certain way.
- Do not pile `///` on private, self-explanatory helper functions.

**Write "why", not "what":**

- Type names, field names, and function names already say *what*; comments should explain *why* and document *invariants*.
  - Good: `// Serialize as untagged because most providers omit the type field`
  - Bad: `// This is an enum representing assistant content`
- Document **invariants**, **preconditions**, **side effects**, and **protocol contracts**.
- Never repeat facts already obvious from the signature, type, or naming.

**Brevity first:**

- If one line suffices, do not write two. Trivial setters need no comment or at most a single sentence.
- Avoid polite filler: no "This function returns …". Start with an imperative or noun phrase: "Returns …", "Builds …".
- First line is a standalone summary; expand after a blank line if needed.

**Conventional rustdoc sections** (use when they add value):

- `# Errors` — for functions returning `Result`: list failure conditions.
- `# Panics` — for functions that can panic: list trigger conditions.
- `# Safety` — for `unsafe fn`: state invariants the caller must uphold.
- `# Examples` — typical usage in ```` ```rust ```` blocks, runnable by `cargo test --doc`.

**Prohibited patterns:**

- No bare `TODO` without owner and context.
- No commented-out old code — use git history.
- No timestamps, author names, or changelog-style comments — VCS handles that.
- No "fixes issue #123" in comments — put that in the PR description.
- No restating the type signature in comments.

### 3.2 Module Organization: no `mod.rs`

Use the Rust 2018+ recommended layout: parent modules are `.rs` files with matching directories for child modules. Never create a `mod.rs`; flag any encountered during code review.

Rationale: avoids identically-named `mod.rs` files; makes editor tabs more readable; aligns with `rustfmt` and `cargo new` defaults.

**Exception**: `tests/common/mod.rs` — cargo's official convention for sharing helpers across integration tests.

### 3.3 Dependency Management

- All third-party dependencies declare their version in `[workspace.dependencies]`; crates reference them via `dep_name = { workspace = true }` — never pin versions in sub-crates.
- Before adding a dependency, grep `Cargo.toml` to check whether an equivalent crate already exists (e.g. do not add `simd-json` when `serde_json` is already present).
- Do not bump a declared dependency's major version without discussion.
- Feature flags are enabled centrally in the workspace declaration; sub-crates should not repeat `features = [...]` unless genuinely extending them.

### 3.4 Error Handling

- **Library crates**: define named `enum` error types with `thiserror`. Each crate owns its error enum and wraps upstream errors via `#[from]` — do not reuse error enums across crate boundaries.
- **Binaries**: may use `anyhow::Result` for ergonomic error propagation.
- Library code must **not** use `unwrap()` / `expect()` / `panic!()` unless a comment proves the condition is guaranteed unreachable by the type system (prefer `unreachable!()` with an explanation).
- Error messages target developers: include failure context and relevant variable values; avoid "something went wrong" style messages.
- Prefer `?` propagation; do not rewrite `?`-eligible code with `match` + immediate `return Err(...)`.

### 3.5 Pre-commit Checks

Every Rust component must pass these before committing:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo doc --workspace --no-deps   # required when changing public API or doc comments
```

- Clippy warnings are denied by default. To allow a specific lint, use `#[allow(clippy::xxx)]` at the narrowest scope with a comment explaining why.
- Never comment out tests or remove assertions to pass checks — find and fix the root cause.

## 4. Python Conventions

> Detailed Python standards are in [`src/agent-sec-core/AGENTS.md`](src/agent-sec-core/AGENTS.md).

Summary:

- **Version**: Python 3.11.6 (pinned)
- **Package manager**: [uv](https://docs.astral.sh/uv/)
- **Formatting**: black + isort (`line-length = 100`)
- **Linting**: [ruff](https://docs.astral.sh/ruff/) (F, E, W, I, TID252, ANN, S-subset, etc.)
- **Type annotations**: required on all function parameters and return types
- **Imports**: absolute only (`from agent_sec_cli.xxx import yyy`); no relative imports
- **Testing**: pytest; tests live in `tests/` not inside package directories

## 5. TypeScript Conventions

> Detailed config in `src/copilot-shell/`.

- **Linting**: ESLint
- **Formatting**: Prettier
- **Build**: `make build` (npm-based)
- **Test**: `make test`

## 6. Commit Message Rules

> **scope is mandatory** — CI will error if scope is missing.

### Subject line

Format: `type(scope): imperative description`
- **50 characters max** (type + scope + colon + space + description)
- Language: **English only**
- Imperative mood ("add", "fix", "remove" — not "added", "fixes", "removing")
- Lowercase first letter, no trailing period
- Breaking changes: append `!` before colon, e.g. `feat(cosh)!: remove legacy flag`

### Body (when non-trivial)

Separated from subject by a blank line. Cover three things:
1. What architectural choice was made
2. Why this approach over alternatives
3. Known limitations or trade-offs

Do **not** restate the diff line-by-line or paste design docs.

### Trailers

```
Assisted-by: <tool>:<version>
Signed-off-by: Name <email>
```

`Assisted-by` goes **above** `Signed-off-by`. Omit `Assisted-by` if no AI was involved.

```bash
git commit \
  --trailer "Assisted-by: Qoder:1.7.0" \
  --trailer "Signed-off-by: $(git config user.name) <$(git config user.email)>" \
  -m '...'
```

**Tool identifier detection:**

| Detection method | Tool identifier |
|---|---|
| `$QODER_VERSION` env var | `Qoder:<ver>` |
| `$CLAUDE_CODE_VERSION` env var | `Claude Code:<ver>` |
| Parent process is Qoder.app / QoderWork.app | Read `CFBundleShortVersionString` from app bundle |
| Parent process is Claude.app | `Claude:<ver>` |
| Parent process is Cursor.app | `Cursor:<ver>` |

When generating commits, detect the active tool and fill in the actual version. Do **not** hardcode a fixed string like `Qoder:latest`.

### Atomicity

- One commit = one logical change
- Scope must match the actual files changed
- Every commit in a PR must compile independently
- Squash fixup commits before merge

### Scope Inference (by changed file path)

| Changed path | Scope |
|---|---|
| `src/copilot-shell/` | `cosh` |
| `src/agent-sec-core/` | `sec-core` |
| `src/os-skills/` | `skill` |
| `src/agentsight/` | `sight` |
| `src/tokenless/` | `tokenless` |
| `src/ws-ckpt/` | `ckpt` |
| `src/agent-memory/` | `memory` |
| `src/anolisa/` | `anolisa` |
| `src/skillfs/` | `skillfs` |
| `.github/workflows/` | `ci` |
| `docs/` | `docs` |
| `**/package*.json`, `Cargo.lock`, `*.toml` (dep bumps) | `deps` |
| Other root-level config / scripts / tooling | `chore` |

**Multi-component changes**: use the scope covering the most changed files.

### Examples

```
feat(cosh): add --json flag to config command

Scripts need machine-readable config output; chose flat JSON over
nested to keep parsing trivial. Nested config support tracked in #55.

Assisted-by: Qoder
Signed-off-by: Zhang San <zhangsan@example.com>
```

## 7. Branch Naming

> Recommended convention — not enforced for fork contributors.

```
feature/<scope>/<short-desc>    e.g. feature/cosh/json-output
fix/<scope>/<short-desc>        e.g. fix/sec-core/sandbox-escape
hotfix/<scope>/<short-desc>     e.g. hotfix/skill/broken-load
release/<scope>/vX.Y            e.g. release/cosh/v2.1
```

## 8. PR Description

Use [`.github/pull_request_template.md`](.github/pull_request_template.md) as the base template. Key rules:

- **Description**: 2–5 sentences — what changed, why, key implementation decision
- **Related Issue**: `closes #<n>` or `no-issue: <reason>`
- **Type / Scope**: check all that apply based on the diff
- **Testing**: command used, scope (unit/integration/manual), edge cases verified
- PR title follows commit message format: `type(scope): description`

## 9. Documentation Rules

> **MANDATORY**: All documentation rules — file naming, bilingual conventions, CHANGELOG format, file placement, user-guide standards — are defined in [`specs/documentation-standard.md`](specs/documentation-standard.md). You MUST read that file before creating, renaming, or modifying any documentation file. Non-compliance will be rejected in review.

This section intentionally does not duplicate the spec. Do NOT invent documentation rules from memory or prior context — the spec is the single source of truth.

## 10. Code Standards (General)

- All code and comments must be in **English**
- Do not hide errors or risks — make them visible and actionable
- Every change should not only implement the desired functionality but also improve codebase quality

## 11. Scoped Module Rules

Components with complex architectures maintain their own AGENTS.md for module-specific conventions. **Read the relevant scoped file before contributing to that component.**

| Component | Scoped Rules | Focus |
|-----------|-------------|-------|
| **agentsight** | [`src/agentsight/AGENTS.md`](src/agentsight/AGENTS.md) | eBPF probes, data pipeline architecture, module map, FFI constraints, API endpoints |
| **agent-sec-core** | [`src/agent-sec-core/AGENTS.md`](src/agent-sec-core/AGENTS.md) | Python environment, ruff/black rules, hermes-plugin, capability system |
| **anolisa** | [`src/anolisa/AGENTS.md`](src/anolisa/AGENTS.md) | Workspace structure, crate responsibilities |
| **cosh-ng** | [`src/cosh-ng/AGENTS.md`](src/cosh-ng/AGENTS.md) | 5-crate workspace, security heuristics, PTY testing strategy |
| **skillfs** | [`src/skillfs/AGENTS.md`](src/skillfs/AGENTS.md) | Three-crate layout, dependency exceptions, FUSE e2e testing |

## 11.1 File Placement & Documentation Structure

> **MANDATORY**: See [`specs/documentation-standard.md`](specs/documentation-standard.md) §2–§4 for the complete file placement rules, bilingual naming convention, and component-level file requirements. Do NOT rely on cached or memorized rules — read the spec file directly.

## 12. User Guide Documentation Standards

> **MANDATORY**: See [`specs/documentation-standard.md`](specs/documentation-standard.md) §4.6 for user-guide writing standards including installation priority, content boundaries, framing principles, and bilingual language rules. You MUST comply — skipping this spec and writing docs from assumptions is a blocking review issue.
