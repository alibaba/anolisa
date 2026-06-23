# Changelog

## Unreleased

- add compression toggle with dry-run compare mode (`TOKENLESS_COMPRESSION_ENABLED`, `stats summary --compare`)

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