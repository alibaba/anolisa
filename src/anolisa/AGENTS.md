# AGENTS.md

Engineering conventions for AI collaborators working in this repository.

## Comment Guidelines (Rust)

Follow the Rust style guide and API Guidelines. Write comments that help
readers understand intent faster — not comments that paraphrase code.

### 1. Comment types and placement

- **`//!` module-level docs**: Place at the top of a file/module. One or
  two sentences describing what the module does and when to use it.
- **`///` doc comments**: Required on all public (`pub`) items — structs,
  enums, traits, functions, methods, significant fields, and variants.
  These appear in `cargo doc`.
- **`//` inline comments**: Only where the implementation needs to
  explain *why* something is done a certain way.
- Do not pile `///` on private, self-explanatory helper functions.

### 2. Write "why", not "what"

- Type names, field names, and function names already say *what*;
  comments should explain *why* and document *invariants*.
  - Good: `// Serialize as untagged because most providers omit the type field`
  - Bad: `// This is an enum representing assistant content`
- Document **invariants** (e.g. `NonEmptyVec` guarantees at least one
  element), **preconditions**, **side effects**, and **protocol
  contracts** (e.g. fields that providers expect to be echoed back).
- Never repeat facts that are already obvious from the signature, type,
  or naming.

### 3. Brevity first

- If one line suffices, do not write two. Trivial setters need no comment
  or at most a single sentence.
- Avoid politeness filler: "This function returns …". Start with an
  imperative or noun phrase: "Returns …", "Builds …".
- First line is a standalone summary; expand after a blank line if needed.

### 4. Links and cross-references

- Use intra-doc links to reference other items.
- When mentioning child fields on a parent type, use
  `` [`Field`](Self::field) `` so rustdoc renders a clickable link.

### 5. Conventional doc sections

Use rustdoc section headings as needed; do not force them when they add
no value:

- `# Errors` — for functions returning `Result`: list failure conditions.
- `# Panics` — for functions that can panic: list trigger conditions.
- `# Safety` — for `unsafe fn`: state invariants the caller must uphold.
- `# Examples` — typical usage of public APIs in ```` ```rust ```` blocks,
  runnable by `cargo test --doc`.

### 6. Invariants and protocol fields

- For serialization/protocol fields (`#[serde(...)]`, provider IDs,
  signatures, etc.), explain the field's role in the wire protocol and
  why it must be preserved or echoed.
- When using non-default serde attributes (`skip_serializing_if`,
  `flatten`, `untagged`, etc.), explain the motivation.

### 7. Prohibited patterns

- No bare `TODO` without owner and context; always include the reason
  and the condition under which it should be addressed.
- No commented-out old code — use git history.
- No timestamps, author names, or changelog-style comments — VCS
  handles that.
- No "fixes issue #123" in comments — put that in the PR description.
- No restating the type signature in comments.

### 8. Verification

- Run `cargo check` and `cargo doc --no-deps` before committing to
  ensure no broken intra-doc links.
- Public API crates may enable `#![warn(missing_docs)]` at the crate
  root to enforce coverage.

## Workspace structure and crate responsibilities

## Module organization: no `mod.rs`

Use the Rust 2018+ recommended layout: parent modules are `.rs` files
with matching directories for child modules.

Rationale: avoids a sea of identically-named `mod.rs` files; makes file
trees and editor tabs more readable; aligns with `rustfmt` and
`cargo new` defaults. Never create a `mod.rs`; fix any encountered
during code review.

## Dependency management

- All third-party dependencies declare their version in
  `[workspace.dependencies]`; crates reference them via
  `dep_name = { workspace = true }` — never pin versions in sub-crates.
- Before adding a dependency, grep `Cargo.toml` to check whether an
  equivalent crate already exists (e.g. do not add `simd-json` when
  `serde_json` is already present).
- Do not bump a declared dependency's major version without discussion.
- Feature flags are enabled centrally in the workspace declaration;
  sub-crates should not repeat `features = [...]` unless genuinely
  extending them.

## Error handling

- **Library crates**: Define named `enum` error types with `thiserror`.
  Each crate owns its error enum and wraps upstream errors via `#[from]`
  — do not reuse error enums across crate boundaries.
- **Binaries**: May use `anyhow::Result` for ergonomic error propagation.
- Library code must not use `unwrap()` / `expect()` / `panic!()` unless
  a comment proves the condition is guaranteed unreachable by the type
  system (prefer `unreachable!()` with an explanation in that case).
- Error messages target developers: include failure context and relevant
  variable values; avoid "something went wrong" style messages.
- Prefer `?` propagation; do not rewrite `?`-eligible code with `match`
  + immediate `return Err(...)`.

## Pre-commit checks

### Commit conventions

Follow [Conventional Commits](https://www.conventionalcommits.org/)
style; write commit messages in English.

Trailer format per
[kernel coding-assistants](https://docs.kernel.org/process/coding-assistants.html):

```
<type>(<scope>): <subject>

<body>

Assisted-by: AGENT_NAME:MODEL_VERSION
Signed-off-by: Human Name <email>
```

- `Assisted-by` attributes the AI tool that assisted development, in
  `tool:version` format.
- `Signed-off-by` is added only by human contributors, certifying the
  DCO.
- AI agents MUST NOT add `Signed-off-by`.
- Use `git commit -s` to append the trailer automatically.

### Check commands

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --all-targets
cargo test --workspace
```

- When modifying public APIs or doc comments, additionally run
  `cargo doc --workspace --no-deps` to verify intra-doc links.
- Clippy warnings are denied by default; if there is genuine reason to
  suppress one, use `#[allow(clippy::xxx)]` at the narrowest possible
  scope with a comment explaining why.
- Never comment out tests or remove assertions just to pass checks —
  find and fix the root cause.
