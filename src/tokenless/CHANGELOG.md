# Changelog

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
