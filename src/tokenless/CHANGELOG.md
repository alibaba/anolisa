# Changelog

## Unreleased

- fix Claude Code PostToolUse hook to *replace* the model-visible tool result via `hookSpecificOutput.updatedToolOutput` (Claude Code >= 2.1.121) instead of appending the compressed payload through the additive `additionalContext`, which duplicated the original output — `additionalContext` now carries only additive env-attribution diagnostics, empty schema fields (e.g. Bash `stderr`/`interrupted`/`isImage`) are restored on the replacement, older or undetectable Claude Code versions fail open by disabling compression, and non-beneficial compressions now pass through unchanged for every agent; adds `tests/test-posttooluse-replacement.sh` covering replacement semantics (closes #1645)

## 0.7.1

- fix RPM tarball to exclude generated `.anolisa/component.toml`, ensuring rpmbuild always regenerates the adapter contract from the authoritative `.toml.in` template — previously stale checked-in copies shipped outdated contracts missing claude-code, codex, and cosh adapter declarations (closes #1470)
- synchronize adapter contracts: declare every shipped driver (qoder, claude-code, codex, cosh, qwencode) in `component.toml.in` and add CI check (`check-component-contract`) to keep them in sync
- raise test coverage from 75% to 90%: ~170 new unit tests across all four crates covering compression edge cases, stash round-trip, schema migration, SLS writer, and CLI dispatch
- harden test isolation: replace unsafe env-var mutations with RAII `TempDbGuard` / `EnvGuard` to prevent tests from touching real `~/.tokenless` state; enforce `--test-threads=1` in Makefile (Rust 2024 `set_var` is unsafe)


## 0.7.0

- add MCP `tokenless_retrieve` stdio server (`tokenless mcp serve`) so MCP-connected agents can recover truncated payloads on demand — the MCP analogue of the `tokenless retrieve` CLI, closing the stash MCP gap vs Headroom CCR's `headroom_retrieve`
- complete reversible-compression (stash / CCR) coverage across the remaining lossy paths: `ResponseCompressor` string truncation, `ResponseCompressor` depth truncation, and `SchemaCompressor` description truncation are now stash-backed with `<<tokenless:KEY>>` markers; fit-check before stash prevents orphan entries; shared `stash_suffix()` helpers keep marker budget consistent
- add `--no-stash` / `--stash-db` flags to `compress-schema` (mirroring `compress-response`); dry-run (`compression_on=false`) skips the stash so markers never reach the LLM without a retrievable entry
- add lazy TTL purge to `SqliteStore`: expired rows are physically deleted before retrieve lookups so the stash db does not grow unbounded
- add actual-savings-rate display: `StatsSummary::actual_savings_percent(session_total_tokens)`; `format_summary()` / `format_summary_json()` accept optional session total and emit an "Overall Savings vs Total Consumption" section plus new JSON fields (`session_total_tokens`, `actual_savings_tokens`, `actual_savings_percent`) — backward-compatible when absent
- add stash write/size counters to compression stats (`record_compression_stats` extended); retrieve-side hits/misses deferred pending a stats use case
- add qoder framework driver (qodercli install + settings.json merge/prune, `AdapterOps::read_file`, symlink-safe atomic `write_file`); gate qoder to `adapter_type=plugin`; fail closed on forged receipts and require all managed hooks
- raise test coverage from 59% to 75%: 100+ new unit tests plus 18 CLI integration tests across all four crates, test code moved to `src/tests/` via `include!()` for cleaner separation
- add reversible-compression user manual (`docs/stash-reversible-compression.md`) plus README updates: architecture tree entry for `tokenless-ccr`, retrieve subsection documenting hash/marker input and `--no-stash`/`--stash-db`, scenario-mapping rewrite of the "Applicable Scenarios & Expected Effects" chapter
- rename tokenless docs `*_CN.md` to `*_zh.md`, add bidirectional bilingual links, create `README.md` + `README_zh.md`
- address adapter review findings: trust packaged datadir roots for Codex symlink targets, scope Claude Code marketplaces per component and fail closed, reject framework/adapter type mismatches before enable
- silence clippy warnings surfaced by rustc 1.94 stable in existing tests (`field_reassign_with_default` in tokenless-cli, `bool_assert_comparison` and `default_constructed_unit_structs` in tokenless-stats)

## 0.6.1

- bundle tool_categories.json into dist for npm installs
- use node: prefix, eliminate shell subprocess in openclaw plugin

## 0.6.0

- add absolute saved values + schema version to JSON output
- use import.meta.dirname instead of __dirname in openclaw plugin
- add qwencode adapter for Qwen Code extension
- fix rtk pytest 'No tests collected' regression
- add trusted FHS fallback paths for hook_utils import in codex scripts
- add SLS JSONL data collection with config toggle
- add tokenless RPM component contract (publishing metadata)
- add compression toggle with dry-run compare mode (`TOKENLESS_COMPRESSION_ENABLED`, `stats summary --compare`)
- enable SLS recording by default and document usage
- align compression mode serde/db form and dedup config load
- expand RPM component contract (bundle.entry + hermes)
- make SLS writer append-only and skip when log file absent
- prefer tool_call_id over internal tool_use_id for qwencode hooks
- bump vendored rtk to v0.43.0; rework pytest stderr-surfacing patch for the refactored runner; drop grep-fallback-fix (root cause fixed upstream) and preflight-skip-python (reversed upstream)
- sync toon-format to 0.5.0 in Makefile and spec (was stale at 0.4.6)

## 0.5.1

- add --json output to stats summary
- implement unified tool categorization and 3-layer compression strategy
- add rtk grep fallback pattern fix patch
- add rtk pytest error report patch

## 0.5.0

- add Hermes adapter runner
- drop TOON wrapper prefix and slim diagnostic tags
- unify rtk rewrite exit code 3 handling across adapters
- secure shell variable interpolation in env-fix and hooks
- add subprocess returncode checks and extract shared hook utilities
- secure resolveBinaryPath and improve binary cache invalidation
- use mktemp in tests and safe home expansion
- bound SchemaCompressor recursion to prevent stack overflow
- propagate env-fix subprocess failures instead of returning stdout
- anchor home lookup on getpwuid_r and trust-check candidate binaries
- harden env-fix install paths with uid trust check and divert stderr to log
- recover from poisoned mutex in stats recorder instead of failing
- add input size limit and validate db path
- reserve truncation marker length in response compressor
- rename openclaw plugin Name to Tokenless and ID to tokenless
- add qoder CLI adapter
- compress-schema on array input
- warn when compression is skipped
- stats command syntax
- add Claude Code adapter plugin
- error on TTY stdin instead of hang
- add codex adapter plugin
- fix compression pipeline output inflation, truncation and hook timeouts
- harden env-fix, version extraction, file trust, schema, permissions
- address review findings — trailing newline, chmod guard, rate-limited log, comment
- make env attribution reachable for skip-tools entries
- add selective-claw context engine plugin
- address review findings for selective-claw plugin
- remove invalid "2" dependency from selective-claw
- restore indentation in compress_response_hook.py
- harden hook exit-code handling + trust model consistency
- only warn on truly unexpected rtk exit codes
- dedup rewrite_hook, import from hook_utils

## 0.4.1

- fix version_ge 3-segment truncation in env_check.rs (compare all segments)
- add qoder, claude-code, codex adapter plugins and documentation
- sync manifest.json with template to include all six agents
- update README and user manuals for new agent integrations
- add __pycache__ to root .gitignore
- update response-compression.md with all agent integration paths
- derive Makefile version from Cargo.toml, fix spec changelog weekday
- normalize adapter version numbers to 0.4.0
- derive adapter plugin versions from Cargo.toml instead of hardcoding

## 0.4.0

- correct 5 bugs in stats, naming, SQL, paths and permissions
- align FHS paths, restructure adapter dir, remove install.sh
- address code review findings across schema, env-check, hooks, and plugin
- add hermes agent plugin
- security hardening & critical algorithm correctness
- behavioral correctness & logic fixes
- dedup, dead code removal & cosmetic cleanup
- support staged installs
- support Debian/Ubuntu FHS paths and harden binary resolution
- build OpenClaw plugin to dist/index.js

## 0.3.2

- replace spoofable home-dir uid derivation with libc::getuid() syscall for trust chain integrity
- replace subprocess toon -e calls with in-process toon_format::encode_default() library call
- replace rtk/toon git submodules with crates.io deps and inline toon-format source
- hard-fail on rtk stats patch failure in justfile setup-rtk recipe
- unify compress-toon/compress-schema/compress-response error exit codes (all exit 2)
- remove 2>/dev/null || true from Makefile toon install (hard fail on missing binary)
- remove redundant #[source] attribute on thiserror variants that already have #[from]
- deduplicate Python hook FHS path constants into shared hook_utils module
- add libc to workspace dependencies for uid syscall
- add detailed rust >= 1.89 comment in spec.in explaining CI pin rationale

## 0.3.0

- add tool-ready 4-phase environment pre-check with cosh extension integration
- skip compression and stats when no token savings
- pass caller context to rtk stats via .rewrite-context file
- remove redundant cosh extension install/uninstall from install.sh
- convert cosh hooks to extension format per cosh dev guide
- skip zero compression and stats recording
- use isExecutable() and resolved paths in openclaw plugin
- resolve rtk/toon binary paths for RPM-installed plugins
- correct RPM install paths to align with install.sh expectations
- preserve tool result message structure in TOON encoding
- align install paths with FHS
- auto-record stats with real tool_use_id from hook payload
- restructure RPM dirs and remove auto plugin/hook installation

## 0.2.0

- add compression stats with auto-record from real data
- add TOON context compression support
- skip compression for skill and content-retrieval tools

## 0.1.0

- introduce tokenless into ANOLISA (#199)