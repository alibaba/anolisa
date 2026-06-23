# AGENTS.md

This file documents the engineering conventions that AI collaborators
and human contributors are expected to follow when writing code in
this repository. Read it once before opening your first PR.

---

## 1. Comment principles (Rust)

Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/)
and the official [rustfmt style guide](https://doc.rust-lang.org/nightly/style-guide/).
Write comments that help the reader understand intent faster; do not
restate what the code already says.

### 1.1 Comment kinds and where they go

- `//!` **module-level docs**: at the very top of a file or module, one
  or two sentences stating what the module is responsible for and when
  to use it. Every `src/*.rs` file in this repository already carries a
  `//!` header — keep that habit for new modules.
- `///` **doc comments**: required on every public (`pub`) item —
  structs, enums, traits, functions, methods, important fields,
  variants. These flow into `cargo doc`.
- `//` **plain comments**: only where the implementation needs to
  explain *why* it is written a particular way.
- Do not pile `///` on private helpers whose purpose is obvious from
  the body.

### 1.2 Write the *why*, not the *what*

- Type names, field names and function names already say *what*;
  comments should add the *why* and the *invariants*.
  - Good: `// guarded by `len() > 1` above`
  - Bad: `// take the last element`
- Describe **invariants** (e.g. `emit_at_depth` always contains at
  least one element), **preconditions**, **side effects** and
  **contracts with external protocols**.
- Anything that can be read directly from the signature, type or name
  should not be repeated in a comment.

### 1.3 Brevity first

- One-line suffices over two. Trivial setters get no comment or a
  single sentence.
- Drop polite filler: no `This function returns ...`, no `这是一个用于
  ...`. Lead with an imperative or noun phrase: `Returns ...`,
  `Builds ...`, `Computes ...`.
- The first line is a one-sentence summary on its own; if you need
  more, leave a blank line before the body.

### 1.4 Links and cross-references

- Use intra-doc links to reference other items, for example
  `` [`SkillStore::upsert`] `` or `` [`SkillEvent::Created`] ``.
- When referring to a child field from its parent, use
  `` [`field`](Self::field) `` so rustdoc renders it as a hyperlink.

### 1.5 Conventional rustdoc sections

Use the conventional rustdoc section headings where they apply; do not
invent them just to fill space:

- `# Errors` — for functions that return `Result`, list the conditions
  under which they fail.
- `# Panics` — for functions that can panic, list the conditions that
  trigger the panic.
- `# Safety` — for `unsafe fn`, list the invariants the caller must
  uphold.
- `# Examples` — typical use of a public API, written inside
  ```` ```rust ```` blocks so `cargo test --doc` can execute them.

### 1.6 Don't

- No bare `TODO` (no owner, no context). If you must, include the
  reason and the trigger condition for resolving it.
- No commented-out old code — that is what git history is for.
- No timestamps, author names or in-source changelogs — that is what
  the VCS is for.
- No `Fixes issue #123`-style references in source comments; put them
  in the PR description.
- Do not restate a type signature inside its own doc comment.

---

## 2. Workspace layout

```text
crates/
  skillfs-core/   parser / store / views / compiler / env / watcher
  skillfs-fuse/   FUSE filesystem layer
  skillfs-cli/    `skillfs` binary (mount / classify / validate / list)
docs/specs/       implementation specifications
docs/skills/      the bundled agent skill (skillfs-mount)
scripts/          build.sh, test.sh
```

Each crate's `Cargo.toml` carries a one-line `description = "..."` that
matches the table above; keep them in sync.

---

## 3. Module organisation: no `mod.rs`

We follow the Rust-2018+ non-`mod.rs` layout: a parent module lives in
a `.rs` file with the same stem, and its submodules live inside a
directory of that same name. Do not create `mod.rs` for new modules;
flag and remove them in code review.

**The only exception** is `tests/common/mod.rs` — cargo's official
convention for sharing helpers across integration tests (cargo
deliberately does not treat it as an integration test on its own). The
existing `crates/skillfs-core/tests/common/mod.rs` falls under this
exception.

---

## 4. Dependency management

- Every third-party dependency is declared once in the root
  `Cargo.toml`'s `[workspace.dependencies]`; child crates reference it
  with `dep_name = { workspace = true }`. Never pin a version inside a
  child crate (see §4.1 for the few justified exceptions).
- Before adding a dependency, grep `Cargo.toml` to see whether an
  equivalent crate is already in the tree (e.g. don't add `simd-json`
  when `serde_json` is already there).
- Do not bump the major version of an existing dependency on your own;
  raise it as a separate discussion.
- Enable feature flags centrally in the workspace declaration. Child
  crates should not repeat `features = [...]` unless they truly need
  to extend the workspace set.

### 4.1 Existing exceptions

The two per-crate version literals below are deliberate. Do not
"tidy them up" into `[workspace.dependencies]`:

- `crates/skillfs-core/Cargo.toml :: notify = { version = "7", features = ["macos_kqueue"] }`
  — carries a macOS-specific feature, single consumer.
- `crates/skillfs-cli/Cargo.toml :: clap = { version = "4", features = ["derive"] }`
  — CLI-only; no reason to pollute the workspace.

Any new exception must be justified in the PR description.

---

## 5. Error handling

- **Library crates** (`skillfs-core`, `skillfs-fuse`) use `thiserror`
  to define named `enum` error types. Do not share an error enum
  across crates; wrap upstream errors with `#[from]` instead. The
  current `FuseError` / `ParseError` / `WatcherError` already follow
  this pattern.
- **Binaries** (`skillfs-cli`): clap's built-in error reporting is
  enough; `anyhow::Result` is also fine if it simplifies propagation.
- Library code must **not** use `unwrap()` / `expect()` / `panic!()`
  unless a comment proves the failure case is impossible (either by
  the type system or by a preceding runtime check). The three
  pre-existing exceptions — in `compiler.rs`, `parser.rs`, and
  `cli/main.rs` — each carry a one-line justification; new code must
  do the same, and `unreachable!()` with a comment is preferred over
  bare `unwrap()`.
- Error messages are written for developers: include the relevant
  context and variable values, never `something went wrong`.
- Prefer `?` for propagation. Don't rewrite `?` as `match` +
  `return Err(...)` just to add a branch.

---

## 6. Pre-submission checks

Before committing or opening a PR, run the following from the
workspace root. `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets -- -D warnings` are mandatory
for every code change; do not rely on CI to discover formatting or
lint failures. Every command below must pass before the change is
ready for review (this is the same checklist as
[README §Verification](README.md#verification)):

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
scripts/test.sh                            # e2e FUSE mount; skips itself if fuse3 / /dev/fuse is missing
cargo doc --workspace --no-deps            # intra-doc links; required when public API changes
```

- Clippy warnings are denied by default. To allow a specific lint,
  wrap the narrowest possible scope in `#[allow(clippy::xxx)]` and
  add a comment explaining why.
- Do not silence a failing test or remove an assertion to make the
  checks pass — find the root cause.
- `cargo doc --workspace --no-deps` is **required** when you change
  public API or doc comments; recommended otherwise.

---

## 7. Commit conventions

Inferred from the existing `git log`:

- One-line English subject, ≤ 72 characters, with a conventional
  prefix: `feat:` / `fix:` / `chore:` / `docs:` / `style:` /
  `refactor:` / `test:`. A scope is allowed and encouraged, e.g.
  `docs(spec): ...`, `chore(cargo): ...`.
- The body explains the *why* and the impact. Hard-wrap at 72
  columns.
- Sign off with `Signed-off-by: <name> <email>` on the last line (DCO).
- Keep one logical change per commit — do not bundle fmt, clippy and
  functional changes together.
