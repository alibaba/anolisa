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
```

## Commit Message Rules

> **scope is mandatory** — CI will error if scope is missing.

Format: `type(scope): description`
- Language: **English only**
- `description`: lowercase first letter, no trailing period
- Breaking changes: append `!` before colon, e.g. `feat(cosh)!: remove legacy flag`

### Scope Inference (by changed file path)

| Changed path | Scope |
|---|---|
| `src/copilot-shell/` | `cosh` |
| `src/agent-sec-core/` | `sec-core` |
| `src/os-skills/` | `skill` |
| `src/agentsight/` | `sight` |
| `src/tokenless/` | `tokenless` |
| `src/agent-memory/` | `memory` |
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
fix(sec-core): handle sandbox escape edge case
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

## Code Standards

- All code and comments must be in **English**
- **TypeScript**: ESLint + Prettier (configured in `src/copilot-shell/`)
- **Python**: Ruff + Black (configured in `src/os-skills/` and `src/agent-sec-core/`)
- **Rust**: `cargo fmt` + `cargo clippy -- -D warnings`
- Do not hide errors or risks — make them visible and actionable
- Every change should not only implement the desired functionality but also improve codebase quality
