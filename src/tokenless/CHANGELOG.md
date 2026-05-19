# Changelog

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
