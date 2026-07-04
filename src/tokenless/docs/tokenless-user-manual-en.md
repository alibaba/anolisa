# Token Optimization (tokenless)

tokenless is ANOLISA's token-saving toolkit. Through five complementary strategies — Schema compression, response compression, TOON encoding, command rewriting, and tool-readiness checks — it cuts redundancy from tool definitions, API responses, and command output *before* they enter the LLM context window, reducing both runtime token consumption and retry cost.

- **Schema & response compression**: compresses OpenAI Function Calling tool definitions and API responses, saving ~57% and 26–78% of tokens respectively.
- **TOON encoding**: encodes JSON responses into a token-oriented compact format; structured data saves another 15–40%.
- **Command rewriting**: integrates RTK to filter and rewrite 70+ common CLI outputs, eliminating 60–90% of noise.
- **Tool-readiness check**: checks binary/config/permission/network dependencies before execution, auto-fixes missing ones, and attributes environment-class failures to avoid wasted retries.
- **Statistics & observability**: every compression records char/token savings metrics; supports SQLite aggregation and SLS JSONL ingestion, with dual-run comparison of real savings.

---

## Capability overview

| Strategy | Token savings | Description |
|------|----------|------|
| Schema compression | ~57% | Compresses OpenAI Function Calling tool definitions |
| Response compression | 26–78% | Compresses API / tool responses (drops debug/null/empty, truncates long content) |
| TOON encoding | 15–40% | Lossless compact JSON format, chained after response compression |
| Command rewriting | 60–90% | Filters 70+ CLI outputs via RTK |
| Tool-readiness check | reduces retry waste | Pre-execution env check, auto-fix, failure attribution |

---

## Installation

### Via anolisa CLI (recommended)

```bash
anolisa install tokenless
```

Produces three binaries — `tokenless`, `rtk`, `toon` — plus adapter resources (hooks, plugins, tool-ready-spec, env-fix script).

### RPM package (Alinux 4)

```bash
sudo dnf install tokenless
```

The RPM installs to system-level FHS paths: `/usr/bin/tokenless`, `/usr/libexec/anolisa/tokenless/{rtk,toon}`, `/usr/share/anolisa/adapters/tokenless/`, `/usr/share/anolisa/extensions/tokenless/`. `%post` cleans up stale artifacts (pre-FHS layout, legacy `tokenless-openclaw` plugin id, etc.); the cosh extension is auto-discovered by copilot-shell — no manual `settings.json` edit needed.

### Source build (developers)

```bash
git clone https://github.com/alibaba/anolisa
cd anolisa/src/tokenless

make setup    # build + install binaries + register all adapters
```

Build deps: Rust ≥ 1.89 (edition 2024), just, Git, nodejs+npm (to compile the OpenClaw TS plugin). Runtime deps: python3, jq, bash (RPM Requires). See [Appendix · Install paths](#install-paths).

---

## CLI usage

All subcommands accept `-f <path>` (file) or stdin (pipe). Input cap is 64 MiB.

### Compress tool schema

```bash
# Single schema from file
tokenless compress-schema -f tool.json

# Batch from stdin (JSON array)
cat tools.json | tokenless compress-schema --batch

# With stats tracking
tokenless compress-schema -f tools.json --batch \
  --agent-id copilot-shell --session-id sess-001
```

| Argument | Description |
|------|------|
| `-f, --file <path>` | Input file; omit to read stdin |
| `--batch` | Compress a JSON array (auto-enabled when input is an array) |
| `--agent-id` / `--session-id` / `--tool-use-id` | Stats tracking fields |

### Compress API response

```bash
tokenless compress-response -f response.json

# Or pipe
curl -s https://api.example.com/data | tokenless compress-response

# Override defaults
tokenless compress-response -f resp.json \
  --truncate-strings-at 2048 --truncate-arrays-at 16 --max-depth 6
```

| Argument | Description |
|------|------|
| `-f, --file <path>` | Input file; omit to read stdin |
| `--truncate-strings-at <usize>` | String truncation threshold (default 4096) |
| `--truncate-arrays-at <usize>` | Array truncation threshold (default 32) |
| `--max-depth <usize>` | Nesting depth cap (default 8) |

### TOON encode/decode

```bash
# JSON → TOON (compact format)
echo '{"name":"Alice","age":30}' | tokenless compress-toon

# TOON → JSON
echo 'name: Alice
age: 30' | tokenless decompress-toon

# Round-trip
echo '{"name":"test","value":42}' | tokenless compress-toon | tokenless decompress-toon
```

`compress-toon` also supports `--agent-id`/`--session-id`/`--tool-use-id` for stats. If encoding yields no savings, the CLI emits the original and prints a stderr notice; no stats are recorded.

### Environment readiness check

```bash
# Check a specific tool (alias + case-insensitive)
tokenless env-check --tool Shell

# Check all
tokenless env-check --all

# Full two-level checklist
tokenless env-check --checklist

# Auto-fix missing deps
tokenless env-check --tool Shell --fix

# Machine-readable JSON (for hooks/plugins)
tokenless env-check --all --json
```

| Argument | Description |
|------|------|
| `--tool <name>` | Check one tool (exact key → alias → case-insensitive) |
| `--all` | Check all tools in the spec |
| `--fix` | Auto-install missing/outdated deps (calls `tokenless-env-fix.sh fix-all`) |
| `--checklist` | Print the two-level checklist (tool category → deps) |
| `--json` | Machine-readable JSON |

**Status values**: `READY` (all satisfied) / `PARTIAL` (recommended missing, usable) / `NOT_READY` (required missing, tool unusable) / `UNKNOWN` (not in spec). `NOT_READY` JSON includes a `diagnostic` field formatted `[tokenless:ready] <tool>: NOT_READY — required dependency missing: <bin>. Skip retry.` for hooks to pass to the LLM.

### Statistics & measurement

```bash
# Summary (grouped by operation)
tokenless stats summary
tokenless stats summary --json

# List recent records
tokenless stats list                  # default 20
tokenless stats list -l 50

# Show before/after text for a record
tokenless stats show 42

# Clear all stats
tokenless stats clear --yes

# Show toggle states and their source
tokenless stats status

# Enable/disable stats recording (writes config.json)
tokenless stats enable
tokenless stats disable
```

#### Dual-run comparison (dry-run baseline vs active)

`TOKENLESS_COMPRESSION_ENABLED` controls whether compression is actually applied:

- `1` (default): normal compression; result enters LLM context; recorded as `mode=active`
- `0` (dry-run): compute & record predicted savings (`mode=dryrun`) but **emit original**; context unchanged

Run the same task twice to measure real savings:

```bash
# Run 1: dry-run baseline
TOKENLESS_COMPRESSION_ENABLED=0  <run same task>   # session A
# Run 2: real compression
TOKENLESS_COMPRESSION_ENABLED=1  <run same task>   # session B

# Compare
tokenless stats summary --compare <session-A> <session-B>
tokenless stats summary --compare <session-A> <session-B> --json
```

`--compare` takes exactly two session IDs; mode mismatch emits a stderr warning but comparison proceeds. Records with no token savings are not stored.

> Note: tokenless only measures the compressible content it handles; model inference tokens / real billing tokens are out of scope.

#### SLS log ingestion

In addition to SQLite, each compression can append an **SLS JSONL record** for ilogtail/SLS Logtail.

- **Default on** (`sls_enabled=true`). Toggle via `~/.tokenless/config.json` `sls_enabled` or `TOKENLESS_SLS_ENABLED`.
- **Output path**: default `/var/log/anolisa/sls/ops/tokenless.jsonl`; override via `TOKENLESS_SLS_PATH` (must be under `/var/log/` or `/tmp/`, else fallback + warn).
- **File ownership**: the JSONL file is **owned and lifecycle-managed by the anolisa SLS component** (creation, rotation, deletion). tokenless **does not manage it** — before writing, it checks whether the file exists; **if it exists, append; if not, silently skip**. tokenless never creates, truncates, or deletes the file.
- **Recorded fields**: `component.*`, `tokenless.operation`, `tokenless.session_id`/`tool_use_id`/`source_pid`, `tokenless.compression.{before,after}_{chars,tokens}`, `chars_saved`/`tokens_saved` and percentages. **Metrics only — no original compression text**, no sensitive data.

```bash
# Quick verify: anolisa SLS component or manual file must exist first
mkdir -p /tmp && touch /tmp/tokenless-sls.jsonl
TOKENLESS_SLS_ENABLED=1 TOKENLESS_SLS_PATH=/tmp/tokenless-sls.jsonl \
  tokenless compress-response -f resp.json
tail -n1 /tmp/tokenless-sls.jsonl | jq .
```

---

## Agent framework integration

Once installed, tokenless integrates transparently into multiple Agent frameworks:

| Framework | Integration | Strategies covered |
|------|----------|----------|
| **cosh** (copilot-shell) | PreToolUse/PostToolUse hooks | Tool-ready + rewrite + response + TOON + Schema |
| **OpenClaw** | plugin | rewrite + response + Schema |
| **Hermes** | plugin | Tool-ready + rewrite + response + TOON |
| **Qoder** | plugin | Tool-ready + rewrite + response |
| **Claude Code** | marketplace plugin | Tool-ready + rewrite + response + TOON |
| **Codex** | plugin | Tool-ready + rewrite + response + TOON |
| **Qwen Code** | extension plugin | Tool-ready + rewrite + response + Schema |

### Enable adapters

```bash
# List available frameworks
anolisa adapter scan

# Enable as needed
anolisa adapter enable tokenless cosh         # cosh hooks
anolisa adapter enable tokenless openclaw     # OpenClaw plugin
anolisa adapter enable tokenless hermes       # Hermes plugin
anolisa adapter enable tokenless qoder        # Qoder plugin
anolisa adapter enable tokenless claude-code  # Claude Code plugin
anolisa adapter enable tokenless codex        # Codex plugin
anolisa adapter enable tokenless qwencode     # Qwen Code plugin

# Status
anolisa adapter status tokenless

# Disable
anolisa adapter disable tokenless openclaw
```

Equivalent manual registration in the tokenless source directory:

```bash
make cosh-extension-install    # cosh hooks
make openclaw-install          # OpenClaw plugin
make hermes-install            # Hermes plugin
make qoder-install             # Qoder plugin
make claude-code-install       # Claude Code plugin
make codex-install             # Codex plugin
make qwencode-install          # Qwen Code plugin
```

---

## How it works

### Schema compression

Compresses OpenAI Function Calling tool definitions, stripping redundant descriptions and markdown syntax. Source: `crates/tokenless-schema/src/schema_compressor.rs`.

Defaults:

| Parameter | Default | Description |
|------|------|------|
| `func_desc_max_len` | 256 | Max characters for function descriptions |
| `param_desc_max_len` | 160 | Max characters for parameter descriptions |
| `drop_examples` | true | Drop `examples` fields |
| `drop_titles` | true | Drop `title` fields |
| `drop_markdown` | true | Strip markdown syntax |
| `max_depth` | 32 | Recursion depth cap (schemas tolerate deeper nesting) |

### Response compression (7 rules)

Recursively traverses JSON values applying 7 rules. Source: `crates/tokenless-schema/src/response_compressor.rs`.

| Rule | Name | Condition | Handling | Default threshold |
|------|------|---------|---------|---------|
| R1 | String truncation | length > 4096 chars | UTF-8 safe truncation, append `… (truncated)` | 4096 chars |
| R2 | Array truncation | elements > 32 | Keep first 32, append `<... N more items truncated>` | 32 items |
| R3 | Field drop | key matches blacklist | Remove field entirely | 7 fields |
| R4 | null removal | value is `null` | Delete from object/array | enabled |
| R5 | Empty removal | value is `""`/`[]`/`{}` | Delete from object/array | enabled |
| R6 | Depth truncation | nesting depth > 8 | Replace with `<{type} truncated at depth {N}>` | 8 levels |
| R7 | Primitive preservation | bool/number | Keep as-is | — |

**R3 default blacklist**: `debug`, `trace`, `traces`, `stack`, `stacktrace`, `logs`, `logging`

Example (R3 + R4 + R5):

```json
// Input
{"status":"success","data":{"name":"test","count":42},
 "debug":{"request_id":"abc123"},"trace":"GET /api 200","metadata":null,"tags":[],"extra":""}

// Output
{"status":"success","data":{"name":"test","count":42}}
```

### TOON encoding

TOON (Token-Oriented Object Notation) is a lossless JSON codec that eliminates JSON syntax overhead (quotes, commas, colons, braces) while preserving all data. The CLI links `toon-format` (v0.5) as a library; Python hooks invoke the standalone `toon` binary as a subprocess.

| JSON element | JSON syntax | TOON encoding | Savings |
|-----------|-----------|----------|---------|
| Object keys | `"name":` | length-prefixed raw bytes | 60-80% |
| String values | `"value"` | length-prefixed raw bytes | 10-20% |
| Array separators | `, ` | implicit boundaries | 100% |
| Structural braces | `{}`, `[]` | implicit type markers | 100% |

### Command rewriting (RTK)

RTK intercepts shell commands and rewrites them to output only the key information the Agent needs:

```bash
# Original (wastes many tokens)
ls -la /usr/lib

# RTK rewrite (key info only)
rtk rewrite "ls -la /usr/lib"
```

RTK source is cloned from GitHub (`v0.43.0`) by the justfile, with tokenless-specific patches applied, then built. Supports 70+ commands (cargo/npm/go/pytest, etc.); typical savings 60–90%.

### Tool-readiness check (Tool Ready)

Before each tool call, checks dependencies (binaries, configs, permissions, network). If missing, returns `NOT_READY` + a "Skip retry" hint so the LLM stops retrying. Dependencies are declared in `tool-ready-spec.json`:

```json
{
  "Shell": {
    "required": [{ "binary": "jq", "package": "jq", "manager": "apt" }],
    "recommended": [
      { "binary": "rtk", "version": ">=0.35", "package": "rtk", "manager": "cargo",
        "fallback": [
          { "method": "symlink", "binary": "rtk", "source": "/usr/libexec/anolisa/tokenless/rtk" }
        ]
      }
    ]
  }
}
```

String format `"jq"` is also supported (auto-converted to object). `--fix` calls `tokenless-env-fix.sh fix-all` with the missing-deps JSON array on stdin, then re-checks.

### Chained compression pipeline

In the PostToolUse hook, response compression and TOON encoding run sequentially, maximizing savings in two stages:

```
tool response → ResponseCompressor (lossy) → TOON encode (lossless) → final output
```

Each stage is fail-open (passes through original on failure).

---

## Configuration

### Config file

`~/.tokenless/config.json` (owner-only 0600), fields:

| Field | Default | Description |
|------|------|------|
| `stats_enabled` | `true` | Record compression stats to SQLite |
| `sls_enabled` | `true` | Append SLS JSONL records |
| `compression_enabled` | `true` | Actually apply compression. `false` = dry-run: compute & record predicted savings but emit original |

Manage via CLI: `tokenless stats enable` / `disable` / `status`.

### Environment variables

Priority: **env > config.json > default** (per toggle, independent). Empty values are treated as unset. Booleans: `1`/`true`/`yes` (case-insensitive) → true.

| Variable | Purpose | Constraint |
|---------|------|------|
| `TOKENLESS_STATS_ENABLED` | Override `stats_enabled` | — |
| `TOKENLESS_SLS_ENABLED` | Override `sls_enabled` | — |
| `TOKENLESS_COMPRESSION_ENABLED` | Override `compression_enabled` (dry-run toggle) | — |
| `TOKENLESS_STATS_DB` | Custom stats DB path | Must be under the user's home dir, else ignored + warned |
| `TOKENLESS_SLS_PATH` | Custom SLS JSONL path | Must be under `/var/log/` or `/tmp/`, else fallback |
| `TOKENLESS_TOOL_READY_SPEC` | Custom tool-ready-spec path | Must pass trust-path check |
| `TOKENLESS_ENV_FIX_SCRIPT` | Custom env-fix script path | Must pass trust-path check |
| `TOKENLESS_PACKAGE_MANAGER` | Override package-manager detection (dnf/yum/apt/apk) | Testing |
| `TOKENLESS_AGENT_ID` | Agent identity injected by hooks | Set automatically by cosh-extension.json |

### OpenClaw plugin config (`openclaw.plugin.json`)

| Option | Default | Description |
|------|------|------|
| `rtk_enabled` | `true` | Enable RTK command rewriting |
| `response_compression_enabled` | `true` | Enable response compression |
| `schema_compression_enabled` | `true` | Enable Schema compression |
| `verbose` | `true` | Verbose logging |

**Skip logic**: when RTK is enabled and `toolName === "exec"`, response compression is skipped (avoid double optimization); other tools are compressed automatically — observed ~78% savings on `web_fetch`.

### Degradation behavior

Each hook/plugin degrades independently — if the corresponding binary (`rtk` or `tokenless`) is not installed, that hook is silently skipped without affecting other features:

- **cosh hooks**: any failure point does `exit 0` with no output → original result passes through
- **OpenClaw / Hermes / Qoder / Claude Code / Codex / Qwen Code plugins**: try-catch returns null → original result passes through
- **CLI**: errors go to stderr; callers check exit codes to decide fallback
- **Stats recording**: fail-silent; DB errors never block compression output
- **SLS writes**: fail-silent; stderr warning only

---

## Troubleshooting

### Diagnostic tools

```bash
# Component-level diagnosis + auto-fix
anolisa doctor tokenless --fix

# View detailed install plan
anolisa install tokenless --verbose
anolisa install tokenless --dry-run

# Adapter status
anolisa adapter status tokenless

# Tool-ready checklist
tokenless env-check --all --checklist
```

### Common issues

| Problem | Solution |
|------|---------|
| `No input provided` | stdin is a terminal and no `-f` given; use `echo '...' \| tokenless <cmd>` or `-f <path>` |
| `Input exceeds 64 MiB limit` | Input over the 64 MiB cap; split or truncate |
| `JSON parse error` | Invalid JSON; validate with `jq . < input.json` |
| Original emitted + stderr notice | No compression savings (`after >= before`); normal; not recorded |
| dry-run notice | `TOKENLESS_COMPRESSION_ENABLED=0` or config `compression_enabled=false`; original emitted, prediction recorded |
| `Failed to open database` | `~/.tokenless/` not writable, or `TOKENLESS_STATS_DB` rejected as outside home |
| SLS JSONL not generated | Confirm `TOKENLESS_SLS_ENABLED` is not `0`; `TOKENLESS_SLS_PATH` must be under `/var/log/` or `/tmp/`; file must be pre-created by the anolisa SLS component |
| cosh hook not firing | Confirm `cosh-extension.json` exists in `COSH_EXTENSION_DIR`; restart copilot-shell |
| `jq not installed` | `dnf install jq` / `apt install jq` |
| Command not rewritten | Not all commands have RTK equivalents; test with `rtk rewrite "cmd"` |
| Tool Ready false NOT_READY | Inspect `tool-ready-spec.json`; run `tokenless env-check --tool <name> --fix` |
| adapter enable failed | `anolisa adapter scan` to confirm framework installed; `anolisa adapter status tokenless` for details |
| State out of sync after manual dnf | `anolisa repair tokenless`; or `anolisa forget tokenless` then `anolisa adopt tokenless` |

### State-inconsistency repair

If `dnf remove` / `rpm -e` was used to directly modify an ANOLISA-managed package:

```bash
anolisa repair tokenless         # repair state
anolisa forget tokenless         # clear ANOLISA records only (leave package)
anolisa adopt tokenless          # re-adopt
```

---

## Appendix

### Install paths

`INSTALL_PROFILE` selects the prefix: `user` (default, `~/.local`) or `system` (`/usr`, used by RPM).

| Makefile variable | user (default) | system / RPM |
|------|-------------|-------------|
| `PREFIX` | `~/.local` | `/usr` |
| `BINDIR` (tokenless) | `~/.local/bin` | `/usr/bin` |
| `LIBEXECDIR` (rtk, toon) | `~/.local/libexec/anolisa/tokenless` | `/usr/libexec/anolisa/tokenless` |
| `SHARE_DIR` (adapter resources) | `~/.local/share/anolisa/adapters/tokenless` | `/usr/share/anolisa/adapters/tokenless` |
| `COSH_EXTENSION_DIR` | `~/.copilot-shell/extensions/tokenless` | `/usr/share/anolisa/extensions/tokenless` |

`rtk`/`toon` live in `LIBEXECDIR` and are symlinked into `BINDIR` for PATH discovery. Source build overrides:

```bash
make install INSTALL_PROFILE=system DESTDIR=/staging
make setup                # build + install + register all adapters
make adapter-scan         # list registered adapter capabilities
```

### cosh hook inventory

| Hook event | Script | Function | matcher | timeout |
|----------|------|------|---------|---------|
| PreToolUse | `tool_ready_hook.sh` (bash) | Tool Ready pre-check + auto-fix + skip-retry | `""` (all, sequential) | 10000ms |
| PreToolUse | `rewrite_hook.py` (python3) | RTK command rewriting | `^(Bash\|run_shell_command\|terminal\|Shell\|exec\|process)$` | 5000ms |
| PostToolUse | `compress_response_hook.py` | Response compression + TOON + failure attribution | — | 10000ms |
| BeforeModel | `compress_schema_hook.py` | Schema compression | — | 10000ms |

All hooks receive `TOKENLESS_AGENT_ID=copilot-shell`. Auxiliary files: `hook_utils.py`, `compress_toon_hook.py`, `run-hook.sh`, `tool_categories.json`.

### Key file paths

| Purpose | Path |
|------|---------|
| Response compression algorithm | `crates/tokenless-schema/src/response_compressor.rs` |
| Schema compression algorithm | `crates/tokenless-schema/src/schema_compressor.rs` |
| CLI entry | `crates/tokenless-cli/src/main.rs` |
| env-check implementation | `crates/tokenless-cli/src/env_check.rs` |
| Stats recorder | `crates/tokenless-stats/src/recorder.rs` |
| Config loading | `crates/tokenless-stats/src/config.rs` |
| SLS JSONL writer | `crates/tokenless-stats/src/sls.rs` |
| Home dir resolution | `crates/tokenless-stats/src/home.rs` |
| Adapter manifest | `adapters/tokenless/manifest.json.in` |
| cosh extension manifest | `adapters/tokenless/common/cosh-extension.json` |
| Tool Ready dependency spec | `adapters/tokenless/common/tool-ready-spec.json` |
| Auto-fix script | `adapters/tokenless/common/tokenless-env-fix.sh` |
| Stats database (default) | `~/.tokenless/stats.db` |
| Config file | `~/.tokenless/config.json` |
| SLS JSONL (default) | `/var/log/anolisa/sls/ops/tokenless.jsonl` |
| RPM spec | `tokenless.spec.in` |
| Build orchestration | `justfile` |

### Security model

- **Unforgeable identity source**: home dir resolved via `getpwuid_r(getuid())` (passwd), **not** `$HOME`/`dirs::home_dir()` (user-controllable).
- **Database path validation**: `TOKENLESS_STATS_DB` must canonicalize under the user's real home dir, else ignored + warned; empty home → `/dev/null/.tokenless/stats.db` (safe failure).
- **SLS path validation**: `TOKENLESS_SLS_PATH` must be under `/var/log/` or `/tmp/` with no `..`; canonicalized check prevents symlink escape.
- **Tool Ready trust path**: system prefixes (`/usr/share`, `/usr/libexec`, `/usr/lib/anolisa`, `/usr/local/share`) trusted unconditionally; other paths validate file/parent-dir ownership (current_uid or root) and reject world-writable bits. The shell equivalent in `tool_ready_hook.sh` is kept in sync.
- **Config file permissions**: `~/.tokenless/config.json` is chmod 0600 after write.
- **Input cap**: stdin/file reads capped at 64 MiB to prevent OOM.

### Makefile targets

| Target | Description |
|------|------|
| `make build` | Build tokenless + rtk + toon + OpenClaw plugin |
| `make build-tokenless` | Build tokenless + rtk (via justfile) |
| `make build-toon` | Install toon binary |
| `make build-openclaw-plugin` | Compile OpenClaw TS plugin → `dist/index.js` |
| `make install` | build + install binaries + adapter resources + cosh extension |
| `make setup` | Full setup: `install` + `adapter-install` |
| `make test` | All tests (Rust + hooks) |
| `make lint` / `make fmt` / `make clean` | clippy / fmt / clean |
| `make dist` | Produce source tarball (with pre-patched rtk) |
| `make adapter-scan` | List registered adapter capabilities |
| `make adapter-install` / `-uninstall` | Register/unregister all 7 platforms |
| `make cosh-extension-install` / `-uninstall` | cosh extension |
| `make openclaw-install` / `-uninstall` | OpenClaw plugin |
| `make hermes-install` / `-uninstall` | Hermes plugin |
| `make qoder-install` / `-uninstall` | Qoder plugin |
| `make claude-code-install` / `-uninstall` | Claude Code plugin |
| `make codex-install` / `-uninstall` | Codex plugin |
| `make qwencode-install` / `-uninstall` | Qwen Code plugin |

---

**License**: MIT (tokenless core) + Apache-2.0 (vendored rtk)
**Version**: 0.6.1
**Document version**: 2.1 (aligned with ANOLISA-design user-guide structure)
