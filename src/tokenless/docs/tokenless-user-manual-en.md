# Token-Less User Manual

> LLM token optimization toolkit — Schema/Response Compression + Command Rewriting + TOON Format

**Version**: 0.5.0  
**Source**: https://code.alibaba-inc.com/Agentic-OS/Token-Less  
**RPM Source**: https://code.alibaba-inc.com/alinux/tokenless  
**System Requirements**: Rust 1.89+ (edition 2024), Linux (Alinux 4 recommended), just (build runner)

---

## Table of Contents

1. [Overview](#1-overview)
2. [Core Features](#2-core-features)
3. [System Requirements](#3-system-requirements)
4. [Installation](#4-installation)
   - [RPM Package Installation](#41-method-1-rpm-package-installation-recommended-for-alinux-4)
   - [One-Click Source Installation](#42-method-2-one-click-source-installation)
   - [Installation Script](#43-method-3-installation-script)
   - [Step-by-Step Installation](#44-method-4-step-by-step-installation)
5. [Configuration](#5-configuration)
   - [CLI Usage](#51-cli-usage)
   - [Post-Installation Auto-Configuration (RPM)](#52-post-installation-auto-configuration-rpm)
   - [Copilot Shell Configuration](#53-copilot-shell-configuration)
   - [OpenClaw Plugin Configuration](#54-openclaw-plugin-configuration)
6. [Verification & Testing](#6-verification--testing)
7. [Troubleshooting](#7-troubleshooting)
8. [Appendix](#8-appendix)
   - [Makefile Commands](#81-makefile-commands)
   - [Key File Paths](#82-key-file-paths)
   - [Fail-Open Design](#83-fail-open-design)
   - [Default Configuration](#84-default-configuration)
   - [Source Repositories](#85-source-repositories)

---

## 1. Overview

**Token-Less** is an LLM token optimization toolkit that significantly reduces token consumption through **Schema/Response Compression**, **Command Rewriting**, and **TOON Format** strategies.

### 1.1 Core Value Proposition

| Feature | Savings | Description |
|---------|---------|-------------|
| Schema Compression | ~57% | Compresses OpenAI Function Calling tool definitions |
| Response Compression | ~26–78% | Compresses API/tool responses (varies by content) |
| Command Rewriting | 60–90% | Filters CLI command output via RTK |
| TOON Format | 30–60% | Lossless JSON→binary format, best for structured/tabular data |

### 1.2 Supported Integrations

| Integration | Command Rewriting | Response Compression | Schema Compression | TOON | Tool Ready |
|-------------|-------------------|---------------------|-------------------|------|-----------|
| OpenClaw Plugin | ✅ | ✅ | ✅ | ✅ | ✅ |
| Copilot Shell Hook | ✅ | ✅ | ✅ | ✅ | ✅ |
| Hermes Agent Plugin | ✅ | ✅ | ⏳ | ✅ | ✅ |
| Qoder CLI Plugin | ✅ | ✅ | — | — | ✅ |
| Claude Code Plugin | ✅ | ✅ | — | ✅ | ✅ |
| Codex Plugin | ✅ | ✅ | — | ✅ | ✅ |

### 1.3 Architecture Overview

```
Token-Less/
├── crates/tokenless-schema/   # Core library: SchemaCompressor + ResponseCompressor
├── crates/tokenless-cli/      # CLI binary: tokenless command
├── crates/tokenless-stats/    # Stats recording library (SQLite)
├── adapters/tokenless/        # FHS adapter bundle (cosh, openclaw, hermes, qoder, claude-code, codex)
├── third_party/rtk/           # RTK vendored source (justfile clone+patch)
├── third_party/patches/      # Patches for vendored third_party sources
├── Makefile                   # Unified build system
└── docs/                      # Documentation
```

---

## 2. Core Features

### 2.1 Schema Compressor (SchemaCompressor)

Compresses OpenAI Function Calling tool definitions to reduce structural overhead entering the context window.

**Source Location**: `crates/tokenless-schema/src/schema_compressor.rs`

### 2.2 Response Compressor (ResponseCompressor)

Recursively traverses JSON values and applies **7 compression rules** to reduce token consumption.

**Source Location**: `crates/tokenless-schema/src/response_compressor.rs`

#### 7 Compression Rules

| Rule | Name | Condition | Action | Default Threshold |
|------|------|-----------|--------|-------------------|
| R1 | String Truncation | Length > 4096 bytes | Truncate at UTF-8 boundary, append `… (truncated)` | 4096 bytes |
| R2 | Array Truncation | Elements > 32 | Keep first 32, append `<... N more items truncated>` | 32 elements |
| R3 | Field Deletion | Key matches blacklist | Remove entire field | 7 fields |
| R4 | Null Removal | Value is `null` | Delete from object/array | Enabled |
| R5 | Empty Removal | Value is `""`/`[]`/`{}` | Delete from object/array | Enabled |
| R6 | Depth Truncation | Nesting depth > 8 | Replace with `<{type} truncated at depth {N}>` | 8 levels |
| R7 | Primitive Retention | bool/number | Keep as-is | — |

**R3 Default Blacklist**: `debug`, `trace`, `traces`, `stack`, `stacktrace`, `logs`, `logging`

#### Compression Examples

**Example 1 — Field Deletion + Null Removal + Empty Removal (R3 + R4 + R5)**

Input:
```json
{
  "status": "success",
  "data": { "name": "test", "count": 42 },
  "debug": { "request_id": "abc123", "timing": 0.05 },
  "trace": "GET /api/data 200 OK",
  "metadata": null,
  "tags": [],
  "extra": ""
}
```

Output:
```json
{
  "status": "success",
  "data": { "name": "test", "count": 42 }
}
```

**Example 2 — Array Truncation (R2)**

Input:
```json
[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
```

Output:
```json
[1, 2, 3, "<... 7 more items truncated>"]
```

### 2.3 Command Rewriting (RTK)

Integrates [RTK](https://github.com/rtk-ai/rtk) to filter and rewrite CLI command output, supporting 70+ commands.

| Original Command | Rewritten | Typical Savings |
|-----------------|-----------|-----------------|
| `cargo build` | `rtk build` | ~70% |
| `cargo test` | `rtk test` | ~80% |
| `npm run build` | `rtk build` | ~65% |
| `go test ./...` | `rtk test` | ~75% |
| `python -m pytest` | `rtk test` | ~85% |

### 2.4 TOON Compression (TOON Format)

TOON (Token-Oriented Object Notation) is a **lossless binary JSON codec** that eliminates JSON syntax overhead — quotes, commas, colons, and braces — while preserving all data intact. It is particularly effective for structured and tabular data where syntax overhead dominates content.

**Source Location**: Integrated via `toon-format` crate (crates.io v0.4.6), called directly as a Rust library by the CLI. The standalone `toon` binary is used by Python hooks as a subprocess.

#### How TOON Works

TOON replaces JSON's text-based syntax with a compact binary encoding:

| JSON Element | JSON Syntax | TOON Encoding | Savings |
|-------------|-------------|---------------|---------|
| Object keys | `"name":` (quotes + colon) | Length-prefixed bytes | ~60-80% on key-heavy objects |
| String values | `"value"` (quotes) | Length-prefixed raw bytes | ~10-20% |
| Array separators | `, ` (commas + spaces) | Implicit element boundaries | 100% |
| Structural braces | `{}`, `[]` | Implicit from type tags | 100% |
| Numbers/booleans | Text representation | Compact binary encoding | ~30-50% |

#### Compression Effectiveness by Data Type

| Data Type | Typical TOON Savings | Example |
|-----------|---------------------|---------|
| Tabular/array data | 40-60% | `[{"id":1,"name":"A"},...]` (44% observed) |
| Simple flat objects | 10-20% | `{"name":"Alice","age":30}` (15% observed) |
| Nested schema definitions | 5-15% | Function calling tool schemas |

#### TOON vs Response Compression

| Aspect | Response Compression | TOON Compression |
|--------|---------------------|-----------------|
| Strategy | Lossy (truncation, field deletion) | Lossless (full data preservation) |
| Best for | Verbose API responses, debug-heavy output | Structured tabular data, API results |
| Data integrity | May drop fields/truncate strings | Round-trip safe (encode→decode) |
| Sequential use | Applied first in the pipeline | Applied second (after response compression) |

#### Sequential Compression Pipeline

In the Copilot Shell PostToolUse hook, response compression and TOON are applied **sequentially**:

```
Tool Response → ResponseCompressor (lossy) → TOON Encode (lossless) → Final Output
```

This two-stage pipeline maximizes savings: response compression strips verbose/debug content, then TOON eliminates the remaining JSON syntax overhead.

---

## 3. System Requirements

| Dependency | Version | Purpose | Required |
|------------|---------|---------|----------|
| Rust | >= 1.89 (edition 2024) | Compile tokenless and rtk | Build time only |
| Git | Any | rtk source download (justfile) | Build time only |
| just | Any | Build orchestration (rtk clone+patch) | Build time only |
| jq | Any | Hook script JSON processing | Yes |
| rtk | >= 0.35.0 | Command rewriting | Optional |
| toon | >= 0.4.0 | TOON format compression | Optional |
| tokenless | >= 0.1.0 | Schema/Response compression | Optional |
| sqlite3 | Any | Stats database | Optional |

**Note**: Rust and Git are only required for source compilation. RPM package installation does not require these dependencies.

---

## 4. Installation

### 4.1 Method 1: RPM Package Installation (Recommended for Alinux 4)

#### 4.1.1 Build RPM Package

```bash
# Prepare RPM build environment
rpmdev-setuptree

# Copy source to RPM build directory
cp tokenless-0.1.0.tar.gz ~/rpmbuild/SOURCES/

# Build RPM using spec file
rpmbuild -ba tokenless.spec

# Generated RPM package location
~/rpmbuild/RPMS/x86_64/tokenless-0.1.0-3.alnx4.x86_64.rpm
```

#### 4.1.2 Install RPM Package

```bash
# Install with yum (recommended, auto-resolves dependencies)
sudo yum install ./tokenless-0.1.0-3.alnx4.x86_64.rpm

# Or install directly with rpm
sudo rpm -ivh tokenless-0.1.0-3.alnx4.x86_64.rpm
```

#### 4.1.3 RPM Auto-Configuration

After RPM installation, the following configurations are performed automatically:

1. **Binaries**: Installed to `/usr/bin/tokenless` and `/usr/bin/rtk`
2. **Hook Scripts**: RPM installs to `/usr/share/anolisa/adapters/tokenless/common/hooks/`, source installs to `~/.local/share/anolisa/adapters/tokenless/common/hooks/`
3. **OpenClaw Plugin**: Auto-detected and configured (if OpenClaw is installed)
4. **Copilot Shell**: Auto-detected and configured (if Copilot Shell is installed)

**Verify RPM Installation**:
```bash
# Check binaries
which tokenless
# Output: /usr/bin/tokenless

tokenless --version

# Check hook scripts (RPM installation path)
ls -la /usr/share/anolisa/adapters/tokenless/common/hooks/

# Check OpenClaw plugin configuration
cat ~/.openclaw/openclaw.json | jq '.plugins.allow'
```

### 4.2 Method 2: One-Click Source Installation

```bash
# Clone repository (no submodules needed, rtk is downloaded at build time via justfile)
git clone https://code.alibaba-inc.com/Agentic-OS/Token-Less
cd Token-Less

# Full installation: build + install binaries + deploy OpenClaw plugin + Copilot Shell Hook
make setup
```

### 4.3 Method 3: Installation Script

```bash
# Full setup: build + install + all adapters
make setup

# Install OpenClaw plugin only (requires openclaw CLI)
make openclaw-install

# Install copilot-shell hooks only
make cosh-extension-install
```

### 4.4 Method 4: Step-by-Step Installation

#### 4.4.1 Build

```bash
# Build tokenless + rtk (release mode, rtk cloned+patched via justfile)
make build

# Build tokenless + rtk only
make build-tokenless
```

#### 4.4.2 Install Binaries

```bash
# Install to ~/.local/bin (default)
make install

# Custom installation path
make install BIN_DIR=/usr/local/bin
```

#### 4.4.3 Deploy OpenClaw Plugin

```bash
# Using Makefile
make openclaw-install

# Custom plugin path
make adapter-install

# Manual installation
cp -r adapters/tokenless/openclaw/ /usr/share/anolisa/adapters/tokenless/openclaw/
```

#### 4.4.4 Deploy Copilot Shell Hook

```bash
# Using Makefile
make cosh-extension-install

# Manual installation
mkdir -p ~/.local/share/anolisa/adapters/tokenless/common/hooks
cp adapters/tokenless/common/hooks/*_hook.py ~/.local/share/anolisa/adapters/tokenless/common/hooks/
chmod +x ~/.local/share/anolisa/adapters/tokenless/common/hooks/*_hook.py
```

---

## 5. Configuration

### 5.1 CLI Usage

#### compress-schema

```bash
# Compress single tool schema from file
tokenless compress-schema -f tool.json

# Compress from stdin
cat tool.json | tokenless compress-schema

# Batch compress tools array
tokenless compress-schema -f tools.json --batch
```

#### compress-response

```bash
# Compress API response from file
tokenless compress-response -f response.json

# Compress from stdin
curl -s https://api.example.com/data | tokenless compress-response
```

#### compress-toon

```bash
# Encode JSON to TOON format from file
tokenless compress-toon -f data.json

# Encode from stdin
cat data.json | tokenless compress-toon

# With stats tracking metadata
tokenless compress-toon -f data.json --agent-id my-agent --session-id sess-001
```

#### decompress-toon

```bash
# Decode TOON format back to JSON from file
tokenless decompress-toon -f data.toon

# Decode from stdin
cat data.toon | tokenless decompress-toon

# Round-trip verification
tokenless compress-toon -f data.json | tokenless decompress-toon | jq .
```

### 5.2 Post-Installation Auto-Configuration (RPM)

After RPM installation, the installation script automatically detects and configures installed platforms.

#### 5.2.1 Auto-Detected Platforms

| Platform | Detection Condition | Auto-Configuration |
|----------|---------------------|-------------------|
| OpenClaw | `~/.openclaw/openclaw.json` exists | Plugin deployment + plugins.allow configuration |
| Copilot Shell | `~/.copilot-shell/settings.json` or `~/.qwen-code/settings.json` exists | Hook script deployment + settings.json configuration |

#### 5.2.2 Manual Configuration Trigger

If OpenClaw plugin installation is needed after RPM installation, run:

```bash
# Install OpenClaw plugin (requires openclaw CLI)
/usr/share/anolisa/adapters/tokenless/openclaw/scripts/install.sh
```

#### 5.2.3 Verify Auto-Configuration

```bash
# Check OpenClaw plugin configuration
cat ~/.openclaw/openclaw.json | jq '.plugins.allow'
# Should contain "tokenless"

# Check Copilot Shell Hook configuration
cat ~/.copilot-shell/settings.json | jq '.hooks | keys'
# Should contain PreToolUse, PostToolUse, BeforeModel

# Check hook scripts
ls -la /usr/share/anolisa/adapters/tokenless/common/hooks/
```

### 5.3 Copilot Shell Configuration

#### 5.3.1 Hook Script Locations

Hook script locations depend on the installation method:

| Installation Method | Hook Script Location |
|---------------------|---------------------|
| RPM Installation | `/usr/share/anolisa/adapters/tokenless/common/hooks/` |
| Source Installation | `~/.local/share/anolisa/adapters/tokenless/common/hooks/` |

| Script | Function | Hook Event |
|--------|----------|------------|
| `rewrite_hook.py` | Command rewriting | PreToolUse |
| `compress_response_hook.py` | Response + TOON compression pipeline | PostToolUse |
| `compress_schema_hook.py` | Schema compression | BeforeModel |

#### 5.3.2 Configure settings.json

Edit `~/.copilot-shell/settings.json` (or `~/.qwen-code/settings.json`):

**RPM Installation Configuration**:
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Shell",
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/anolisa/adapters/tokenless/common/hooks/rewrite_hook.py",
            "name": "tokenless-rewrite",
            "timeout": 5000
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/anolisa/adapters/tokenless/common/hooks/compress_response_hook.py",
            "name": "tokenless-compress-response",
            "timeout": 10000
          }
        ]
      }
    ],
    "BeforeModel": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "/usr/share/anolisa/adapters/tokenless/common/hooks/compress_schema_hook.py",
            "name": "tokenless-compress-schema",
            "timeout": 10000
          }
        ]
      }
    ]
  }
}
```

**Source Installation Configuration**:
```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Shell",
        "hooks": [
          {
            "type": "command",
            "command": "~/.local/share/anolisa/adapters/tokenless/common/hooks/rewrite_hook.py",
            "name": "tokenless-rewrite",
            "timeout": 5000
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "~/.local/share/anolisa/adapters/tokenless/common/hooks/compress_response_hook.py",
            "name": "tokenless-compress-response",
            "timeout": 10000
          }
        ]
      }
    ],
    "BeforeModel": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "~/.local/share/anolisa/adapters/tokenless/common/hooks/compress_schema_hook.py",
            "name": "tokenless-compress-schema",
            "timeout": 10000
          }
        ]
      }
    ]
  }
}
```

> **Tip**: RPM installation automatically configures settings.json, no manual editing required.

#### 5.3.3 Hook Workflows

**Command Rewriting (PreToolUse)**:
```
copilot-shell triggers PreToolUse 
  → Hook reads stdin JSON 
  → Calls rtk rewrite 
  → Returns rewritten command
```

**Response Compression (PostToolUse)**:
```
copilot-shell triggers PostToolUse 
  → Hook reads tool_response 
  → Step 1: tokenless compress-response (lossy — removes debug, nulls, truncates)
  → Step 2: tokenless compress-toon (lossless — eliminates JSON syntax overhead)
  → Both steps are fail-open: failures at any stage pass original content through
  → Returns compressed content as additionalContext
```

**Schema Compression (BeforeModel)**:
```
copilot-shell triggers BeforeModel 
  → Hook reads llm_request.tools 
  → Calls tokenless compress-schema --batch 
  → Returns compressed tools array
```

> **Note**: Schema compression is currently a functional placeholder, waiting for anolisa protocol extension to include `tools` in LLMRequest.

### 5.4 OpenClaw Plugin Configuration

#### 5.4.1 Configuration File

Edit `openclaw.plugin.json`:

```json
{
  "rtk_enabled": true,
  "schema_compression_enabled": true,
  "response_compression_enabled": true,
  "toon_compression_enabled": false,
  "skip_tools": [],
  "verbose": false
}
```

| Option | Default | Description |
|--------|---------|-------------|
| `rtk_enabled` | `true` | Enable RTK command rewriting |
| `schema_compression_enabled` | `true` | Enable tool schema compression |
| `response_compression_enabled` | `true` | Enable tool response compression |
| `toon_compression_enabled` | `false` | Enable TOON format compression (sequential after response compression) |
| `skip_tools` | `[]` | Tool names to skip during compression (e.g. `["Read","Glob"]`) |
| `verbose` | `false` | Output detailed logs |

#### 5.4.2 Integration Details

**Response Compression Skip Logic**:
- When RTK is enabled and `toolName === "exec"`, skip compression (avoid double optimization)
- Automatically compress results from all other tool types (`web_search`, `web_fetch`, `read_file`, etc.)
- Observed savings: `web_fetch` approximately **~78%**

**Hook Events**:
| Hook | Event | Function |
|------|-------|----------|
| Command rewriting | `before_tool_call` | Rewrite `exec` commands to RTK equivalents |
| Schema compression | `before_tool_register` | Compress tool schemas |
| Response compression | `tool_result_persist` | Compress tool responses |
| TOON compression | `tool_result_persist` | Sequential TOON encoding (if enabled) |

### 5.5 Hermes Agent Plugin Configuration

The Hermes plugin activates automatically when listed in `~/.hermes/config.yaml`:

```yaml
plugins:
  enabled:
    - tokenless
```

Or enable via CLI:

```bash
hermes plugins enable tokenless
```

The plugin hooks into three Hermes events:

| Strategy | Event | Action |
|---|---|---|
| Tool Ready | `pre_tool_call` | Environment readiness pre-check with auto-fix and skip-retry feedback |
| Command rewriting | `pre_tool_call` | Blocks original command, suggests RTK-rewritten version |
| Response compression | `transform_tool_result` | Compresses tool results via `tokenless compress-response` |
| TOON encoding | `transform_tool_result` | Pipeline step after response compression |

> **Note**: Hermes's `pre_tool_call` hook can only block tool execution (not modify arguments), so command rewriting adds one extra round-trip.

### 5.6 Qoder CLI Plugin Configuration

Install via Makefile:

```bash
make qoder-install
```

Hooks are merged into `~/.qoder/settings.json` automatically. The plugin uses shared hook scripts from the common/hooks directory, referenced via the `${QODER_TOKENLESS_HOOKS}` variable.

### 5.7 Claude Code Plugin Configuration

Install via Makefile or the official `claude plugin` CLI:

```bash
make claude-code-install
```

The adapter exposes a local `anolisa` marketplace containing the `tokenless@anolisa` plugin. Claude Code v2 requires marketplace registration before plugin installation. The `run-hook.sh` dispatcher locates shared hook scripts via FHS paths.

### 5.8 Codex Plugin Configuration

Install via Makefile:

```bash
make codex-install
```

The plugin registers four Codex hooks:

| Event | Action |
|---|---|
| `SessionStart` | Verifies tokenless CLI is installed (non-blocking) |
| `PreToolUse` (tool-ready) | Environment readiness pre-check with auto-fix |
| `PreToolUse` (rewrite) | Shell command rewriting via RTK |
| `PostToolUse` | Response compression + TOON encoding + env error classification |

> **Codex Protocol Constraint**: PostToolUse hooks cannot suppress the original tool output. The plugin injects a compressed summary as `additionalContext`.

---

## 6. Verification & Testing

### 6.0 Real-World Test Results

#### 6.0.1 Test Methodology

**Response Compression Test Script:**

```bash
#!/usr/bin/bash
# Test tokenless-compress-response with mock input

# Build a long tool response (>200 bytes threshold)
LONG_STDOUT=""
for i in $(seq 1 50); do
  LONG_STDOUT="${LONG_STDOUT}This is line $i of verbose output from a tool execution with lots of text to compress.\n"
done

MOCK_RESPONSE="{\"stdout\":\"${LONG_STDOUT}\",\"stderr\":\"\",\"exit_code\":0}"
INPUT="{\"tool_name\":\"run_shell_command\",\"tool_response\":${MOCK_RESPONSE}}"

echo "=== Original response size: ${#INPUT} bytes ==="

RESULT=$(echo "$INPUT" | bash /root/.copilot-shell/hooks/tokenless/compress_response_hook.py 2>/dev/null)

echo "=== Result ==="
echo "$RESULT" | jq '.'

echo ""
echo "=== Compressed context size: $(echo "$RESULT" | jq -r '.hookSpecificOutput.additionalContext // empty' | wc -c) bytes ==="
echo "=== suppressOutput: $(echo "$RESULT" | jq '.suppressOutput') ==="
```

**Test Setup:**
- Generated 50 lines of verbose command output
- Simulated run_shell_command tool response
- Measured original vs compressed size
- Verified hook output format

#### 6.0.2 Test Results

| Metric | Value |
|--------|-------|
| Original Response Size | 4480 bytes |
| Compressed Size | 625 bytes |
| **Savings Ratio** | **~86%** |
| suppressOutput | true (original output suppressed) |

#### 6.0.3 Production Verification

**Hook Execution Logs:**
```bash
# Check compress-response hook triggers
grep "tokenless-compress-response\|compress-response\|compressed response" ~/.copilot-shell/debug/*.log | head -10

# Output: 3 matches found - hook is being triggered correctly
```

**PostToolUse Hook Execution Count:**
```bash
# Check PostToolUse hook execution
grep "firePostToolUseEvent\|PostToolUse.*completed" ~/.copilot-shell/debug/*.log | head -20

# Output: 16 matches - PostToolUse hook firing correctly
# Note: compress-response only triggered 3 times because hook skips responses < 200 bytes
```

**Verification Conclusion:**
- ✅ tokenless-compress-response hook is fully functional
- ✅ Hook skips short responses (< 200 bytes) as designed (fail-open optimization)
- ✅ Actual compression ratio matches expected ~86% savings

---

### 6.1 Manual Hook Testing

```bash
# Test command rewriting (source directory)
echo '{"tool_input":{"command":"cargo test"}}' | bash adapters/tokenless/common/hooks/rewrite_hook.py

# Test response compression (source directory)
echo '{"tool_name":"Shell","tool_response":"{\"stdout\":\"lots of verbose output here...\"}"}' | bash adapters/tokenless/common/hooks/compress_response_hook.py

# Test schema compression (source directory)
echo '{"llm_request":{"tools":[{"name":"test","description":"A test tool","parameters":{}}]}}' | bash adapters/tokenless/common/hooks/compress_schema_hook.py

# Test installed hook (RPM installation)
echo '{"tool_input":{"command":"cargo test"}}' | python3 /usr/share/anolisa/adapters/tokenless/common/hooks/rewrite_hook.py
```

### 6.2 CLI Testing

```bash
# Create test file
echo '{"status":"success","data":{"items":[1,2,3]},"debug":{"id":"abc"}}' > test.json

# Compress response
tokenless compress-response -f test.json

# Compress schema
echo '[{"name":"Shell","description":"Run shell commands","parameters":{"type":"object"}}]' | tokenless compress-schema

# TOON encode
echo '{"users":[{"id":1,"name":"Alice"},{"id":2,"name":"Bob"}]}' | tokenless compress-toon

# TOON round-trip verification
echo '{"name":"test","value":42}' | tokenless compress-toon | tokenless decompress-toon

# Verify compression via stats database
tokenless compress-toon -f large_data.json --agent-id test
tokenless stats list              # List recent compression records
tokenless stats show <record-id>  # Show before/after text for a record
tokenless stats summary           # Aggregate savings across all operations
```

#### Compression toggle and dry-run comparison (dual-run)

Control whether compression is actually applied via `TOKENLESS_COMPRESSION_ENABLED` (or `compression_enabled` in `~/.tokenless/config.json`):

- `1` (default): normal compression; the compressed output reaches the LLM context, recorded with `mode=active`.
- `0` (**dry-run mode**): compression is computed and the predicted savings are recorded (`mode=dryrun`), but the **original text is emitted**, leaving the LLM context uncompressed.

Run the same task twice (once with compression off, once on) to produce a comparison chart that accurately reflects tokenless savings:

```bash
# Run 1: compression off (dry-run baseline, emits original)
TOKENLESS_COMPRESSION_ENABLED=0  <run the same task>   # recorded under session A
# Run 2: compression on (real compression)
TOKENLESS_COMPRESSION_ENABLED=1  <run the same task>   # recorded under session B

# Comparison chart: baseline = context reached (raw), tokenless = compressed
tokenless stats summary --compare <session-A> <session-B>
tokenless stats summary --compare <session-A> <session-B> --json   # machine-readable
```

> Note: tokenless only measures the compressible content it processes; model reasoning tokens / real billed tokens are out of scope.

#### SLS Log Recording (JSONL) {#sls-log-recording}

In addition to SQLite stats, tokenless can append each compression as an **SLS (Simple Log Service) JSONL record** to a file, for ingestion by a log collector (e.g. ilogtail / SLS Logtail).

> **File ownership**: the SLS JSONL file is owned and lifecycle-managed by the **anolisa SLS component** (it creates, rotates, and removes it). tokenless **does not manage the log file** — on each write it first checks whether the file exists, and **appends only if it exists, silently skipping otherwise** (treated as "SLS collection not ready"). tokenless never creates, truncates, or deletes the file or its directory.

- **Enabled by default.** Toggle field: `sls_enabled` in `~/.tokenless/config.json` (default `true`); the `TOKENLESS_SLS_ENABLED` env var takes precedence — `1`/`true`/`yes` turns it on, anything else turns it off.
- **Output path**: defaults to `/var/log/anolisa/sls/ops/tokenless.jsonl`, overridable via `TOKENLESS_SLS_PATH`. The path must live under `/var/log/` or `/tmp/` and must not contain `..`, otherwise it falls back to the default with a warning. Prefer `/var/log/` in production (`/tmp/` is world-writable).
- **Recorded fields** (dot-namespaced keys matching the SLS ingestion schema):
  - `component.name` / `component.version` / `component.agent_name` (agent identity, not user identity)
  - `tokenless.operation` (e.g. `compress-schema` / `compress-response` / `compress-toon` / `rewrite-command`)
  - `tokenless.session_id` / `tokenless.tool_use_id` / `tokenless.source_pid` (optional, omitted when empty)
  - `tokenless.compression.{before,after}_{chars,tokens}`, `chars_saved`, `tokens_saved`, and both percentages

  > Only metrics (char/token counts) are recorded — **no compressed payload** and no sensitive data; records carry no timestamp / hostname / user identity.

```bash
# Show current toggle state and source (default / config file / env override)
tokenless stats status

# Quick check: the file must first exist (created by the anolisa SLS component)
# before tokenless will write to it.
mkdir -p /tmp && touch /tmp/tokenless-sls.jsonl
TOKENLESS_STATS_ENABLED=1 TOKENLESS_SLS_ENABLED=1 \
TOKENLESS_SLS_PATH=/tmp/tokenless-sls.jsonl \
tokenless compress-response -f resp.json
tail -n1 /tmp/tokenless-sls.jsonl | jq .
```

**Disabling SLS**: set `TOKENLESS_SLS_ENABLED=0`, or `"sls_enabled": false` in `~/.tokenless/config.json`.

**Production notes**:
- tokenless does not create the log file or directory — the file must be pre-created by the anolisa SLS component; if it is absent, tokenless skips the write.
- Log rotation is handled by the anolisa SLS component; tokenless does not touch the file lifecycle.
- Writes are fail-silent (stderr-only warning, never affects the main flow).

### 6.3 Verify Installation

```bash
# Check binaries
which tokenless
which rtk

# Check versions
tokenless --version
rtk --version

# Check hook scripts (RPM installation)
ls -la /usr/share/anolisa/adapters/tokenless/common/hooks/

# Check hook scripts (Source installation)
ls -la ~/.local/share/anolisa/adapters/tokenless/common/hooks/
```

---

## 7. Troubleshooting

### 7.1 Copilot Shell Hook

| Problem | Solution |
|---------|----------|
| Hook not firing | Check `settings.json` path, restart Copilot Shell |
| `jq not installed` | Install jq: `apt install jq` (Linux) or `brew install jq` (macOS) |
| `rtk too old` | Upgrade: `cargo install rtk` |
| Command not rewritten | Not all commands have RTK equivalents, test with `rtk rewrite "cmd"` directly |
| `tokenless not installed` | Run `make install` |
| Response not compressed | Responses < 200 bytes are skipped (not worth compressing) |
| Schema compression not active | Waiting for anolisa protocol to add `tools` to LLMRequest |
| JSON parse error | Validate JSON format with `jq . < settings.json` |
| TOON encode fails | Ensure `toon` binary is in PATH; only JSON input is supported |
| TOON stats not recorded | Verify `TOKENLESS_STATS_ENABLED` is not set to `0` or `false` |
| SLS JSONL not generated | Verify `TOKENLESS_SLS_ENABLED` is not `0`/`false` (on by default); confirm `TOKENLESS_SLS_PATH` is under `/var/log/` or `/tmp/`; check stderr `tokenless-sls:` warnings; ensure the directory is writable |

### 7.2 OpenClaw Plugin

| Problem | Solution |
|---------|----------|
| Plugin not loaded | Check plugin path: `~/.openclaw/plugins/tokenless/` |
| RTK not working | Ensure `rtk` is in `$PATH`, check `rtk_enabled` configuration |
| Compression not working | Check `response_compression_enabled` configuration |
| TOON compression not working | Check `toon_compression_enabled` configuration, ensure `toon` binary in PATH |
| Timeout | Plugin timeout is 2-3 seconds, complex operations may timeout and skip |

### 7.3 General Issues

```bash
# Rebuild and reinstall
make clean && make build && make install

# Check dependencies
cargo --version
git --version
jq --version

# View logs
# OpenClaw: Set verbose: true for detailed logs
# Copilot Shell: Check ~/.copilot-shell/logs/
```

---

## 8. Appendix

### 8.1 Makefile Commands

| Command | Function |
|---------|----------|
| `make build` | Build tokenless + rtk |
| `make build-tokenless` | Build tokenless + rtk (via justfile) |
| `make build-toon` | Install TOON binary via `cargo install toon-format` |
| `make install` | Install binaries to BIN_DIR (default: ~/.local/bin) |
| `make test` | Run tests |
| `make lint` | Run clippy checks |
| `make fmt` | Format code |
| `make clean` | Clean build artifacts |
| `make adapter-install` | Install all adapters (cosh+openclaw+hermes+qoder+claude-code+codex) |
| `make openclaw-install` | Install OpenClaw plugin |
| `make openclaw-uninstall` | Uninstall OpenClaw plugin |
| `make hermes-install` | Install Hermes Agent plugin |
| `make hermes-uninstall` | Uninstall Hermes Agent plugin |
| `make qoder-install` | Install Qoder CLI plugin |
| `make qoder-uninstall` | Uninstall Qoder CLI plugin |
| `make claude-code-install` | Install Claude Code plugin |
| `make claude-code-uninstall` | Uninstall Claude Code plugin |
| `make codex-install` | Install Codex plugin |
| `make codex-uninstall` | Uninstall Codex plugin |
| `make cosh-extension-install` | Install Copilot Shell Hook |
| `make cosh-extension-uninstall` | Uninstall Copilot Shell Hook |
| `make setup` | Full installation: build + install + adapter deployment |

### 8.2 Key File Paths

| Purpose | File Path |
|---------|-----------|
| Core compression algorithm | `crates/tokenless-schema/src/response_compressor.rs` |
| Schema compression | `crates/tokenless-schema/src/schema_compressor.rs` |
| CLI subcommand | `crates/tokenless-cli/src/main.rs` |
| Stats recorder (SQLite) | `crates/tokenless-stats/src/recorder.rs` |
| Stats record types | `crates/tokenless-stats/src/record.rs` |
| OpenClaw plugin | `adapters/tokenless/openclaw/dist/index.js` |
| OpenClaw plugin config | `adapters/tokenless/openclaw/openclaw.plugin.json` |
| Copilot Hook — rewrite | `adapters/tokenless/common/hooks/rewrite_hook.py` |
| Copilot Hook — compress response | `adapters/tokenless/common/hooks/compress_response_hook.py` |
| Copilot Hook — compress schema | `adapters/tokenless/common/hooks/compress_schema_hook.py` |
| Tool Ready hook | `adapters/tokenless/common/hooks/tool_ready_hook.sh` |
| Hermes plugin | `adapters/tokenless/hermes/__init__.py` |
| Qoder plugin hooks | `adapters/tokenless/qoder/hooks.json` |
| Claude Code plugin | `adapters/tokenless/claude-code/hooks/run-hook.sh` |
| Codex compression hook | `adapters/tokenless/codex/scripts/compress-response` |
| Tool dependency spec | `adapters/tokenless/common/tool-ready-spec.json` |
| Auto-fix script | `adapters/tokenless/common/tokenless-env-fix.sh` |
| TOON codec (crates.io toon-format) | `toon-format` crate v0.4.6 |
| Stats database (default) | `~/.tokenless/stats.db` |
| Config file (stats/SLS/compression toggles) | `~/.tokenless/config.json` |
| SLS JSONL output (default) | `/var/log/anolisa/sls/ops/tokenless.jsonl` |
| Integration tests | `crates/tokenless-schema/tests/integration_test.rs` |
| TOON E2E tests | `tests/test-toon-full.sh` |
| Full test suite | `tests/run-all-tests.sh` |

### 8.3 Fail-Open Design

All integration paths use **fail-open** strategy:

- **OpenClaw Plugin**: try-catch returns null → original result passes through
- **Copilot Shell Hook**: Any failure point exits with `exit 0` and no output → original result passes through
- **CLI**: Errors output to stderr, caller checks exit code to decide fallback

### 8.4 Default Configuration

| Parameter | Default | Builder Method |
|-----------|---------|----------------|
| `truncate_strings_at` | 4096 | `with_truncate_strings_at(len)` |
| `truncate_arrays_at` | 32 | `with_truncate_arrays_at(len)` |
| `drop_nulls` | true | `with_drop_nulls(bool)` |
| `drop_empty_fields` | true | `with_drop_empty_fields(bool)` |
| `max_depth` | 8 | `with_max_depth(depth)` |
| `add_truncation_marker` | true | `with_add_truncation_marker(bool)` |
| `drop_fields` | 7 fields | `add_drop_field(field)` |

### 8.5 Source Repositories

| Project | URL |
|---------|-----|
| Token-Less Source | https://code.alibaba-inc.com/Agentic-OS/Token-Less |
| RPM Build Source | https://code.alibaba-inc.com/alinux/tokenless |

---

**License**: MIT
**Document Version**: 1.3
**Last Updated**: 2026-06-24
