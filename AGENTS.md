# Instructions for Coding Assistants

This file provides context for AI coding assistants (Qoder, Claude, etc.) working in this repository.

## Project Overview

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

> `agent-sec-core`, `agentsight`, `tokenless`, and `agent-memory` require Linux. Do **not** attempt to build them on macOS or Windows.

## Development Commands

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
```

## Commit Message Rules

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

Use `--trailer` flags (not `-s`) to control ordering:

```bash
git commit \
  --trailer "Assisted-by: Qoder:1.7.0" \
  --trailer "Signed-off-by: $(git config user.name) <$(git config user.email)>" \
  -m '...'
```

**Tool identifier detection** (for reference when writing `Assisted-by`):

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
| `src/agent-memory/` | `memory` |
| `src/anolisa/` | `anolisa` |
| `.github/workflows/` | `ci` |
| `docs/` | `docs` |
| `**/package*.json`, `Cargo.lock`, `*.toml` (dep bumps) | `deps` |
| Other root-level config / scripts / tooling | `chore` |

**Multi-component changes**: use the scope covering the most changed files. PR title follows the same rule.

### Issue Association

If the branch name contains an issue number (e.g. `fix/cosh/42-json-output`), automatically append to commit footer:

```
Closes #42
```

### Examples

```
feat(cosh): add --json flag to config command

Scripts need machine-readable config output; chose flat JSON over
nested to keep parsing trivial. Nested config support tracked in #55.

Assisted-by: Qoder
Signed-off-by: Zhang San <zhangsan@example.com>
```

```
fix(sec-core): handle sandbox escape edge case
feat(sight): add deadloop detection and auto-kill
docs(docs): update installation guide for Linux
chore(ci): pin ubuntu version to 22.04
deps(deps): bump @types/node to 20.11.0
```

## Branch Naming

> Recommended convention — not enforced for fork contributors. CI issues a suggestion, not an error.

```
feature/<scope>/<short-desc>    e.g. feature/cosh/json-output
fix/<scope>/<short-desc>        e.g. fix/sec-core/sandbox-escape
hotfix/<scope>/<short-desc>     e.g. hotfix/skill/broken-load
release/<scope>/vX.Y            e.g. release/cosh/v2.1
```

Fork contributors may use any branch name freely.

## PR Description

When generating a PR description, use `.github/pull_request_template.md` as the base and fill in every section. Rules:

### How to fill each section

**Description** — 2–5 sentences covering:
- What changed and why (motivation)
- Key implementation decision if non-obvious

**Related Issue** — always required:
- Use `closes #<n>` / `fixes #<n>` / `resolves #<n>` so the issue auto-closes on merge
- If no issue exists, write `no-issue: <brief reason>` (typo fix, doc tweak, etc.)

**Type of Change** — check all that apply based on the diff:
- `Bug fix` — patches a defect, no API change
- `New feature` — adds functionality, no breaking change
- `Breaking change` — changes existing behavior (also add `!` in PR title)
- `Documentation update` — docs / comments only
- `Refactoring` — internal restructure, no functional change
- `Performance improvement` — measurable speedup
- `CI/CD or build changes` — workflow / build scripts

**Scope** — check the component(s) whose files were changed:
- `cosh` → any file under `src/copilot-shell/`
- `sec-core` → any file under `src/agent-sec-core/`
- `skill` → any file under `src/os-skills/`
- `sight` → any file under `src/agentsight/`
- `tokenless` → any file under `src/tokenless/`
- `memory` → any file under `src/agent-memory/`
- `anolisa` → any file under `src/anolisa/`
- `Multiple / Project-wide` → cross-component or root-level changes

**Checklist** — mark items that actually apply to this PR; skip items for unaffected components.

**Testing** — describe what was run:
- Command used (e.g. `cd src/copilot-shell && make test`)
- Test scope (unit / integration / manual)
- Any edge cases verified

**Additional Notes** — screenshots, links, caveats, follow-up TODOs.

### PR Title

Same format as commit messages: `type(scope): description`
- Use the scope of the component with the most changes
- Breaking change: `feat(cosh)!: remove legacy config flag`

### Full Example

```markdown
## Description

Add `--json` output flag to the `config` command so scripts can consume
configuration values without text parsing. Returns a JSON object with all
current config keys.

## Related Issue

closes #42

## Type of Change

- [x] New feature (non-breaking change that adds functionality)

## Scope

- [x] `cosh` (copilot-shell)

## Checklist

- [x] I have read the Contributing Guide
- [x] My code follows the project's code style
- [x] I have added tests that prove my fix is effective or that my feature works
- [x] For `cosh`: Lint passes, type check passes, and tests pass
- [x] For `tokenless`: Cargo test passes, clippy warnings resolved
- [x] Lock files are up to date

## Testing

```bash
cd src/copilot-shell && make test
# All 142 tests pass; added 3 new unit tests for --json flag
```

## Additional Notes

Output schema is intentionally flat for now; nested config support tracked in #55.
```

## Changelog Entries

Each user-perceivable change requires a `CHANGELOG.md` entry in the affected component. Follow [Keep a Changelog](https://keepachangelog.com/) format (Added / Changed / Fixed).

1. **One sentence per bullet** — max 25 English words / 40 Chinese characters
2. **User perspective** — describe the behavior change ("X command now supports Y"), not the code change
3. **No internal jargon** — command names and config keys are fine; kernel APIs, framework class names, and syscalls are not
4. **One bullet, one change** — do not combine unrelated changes with "and"
5. **Skip invisible changes** — pure refactors, test infra, and CI tweaks do not belong in the changelog

## Code Standards

- All code and comments must be in **English**
- **TypeScript**: ESLint + Prettier (configured in `src/copilot-shell/`)
- **Python**: Ruff + Black (configured in `src/os-skills/` and `src/agent-sec-core/`)
- **Rust**: `cargo fmt` + `cargo clippy -- -D warnings`
- Do not hide errors or risks — make them visible and actionable
- Every change should not only implement the desired functionality but also improve codebase quality
