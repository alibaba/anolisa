# Token-Less User Manual

> LLM token optimization toolkit — Schema/Response Compression + Command Rewriting

**Version**: 0.1.0  
**Source**: https://code.alibaba-inc.com/Agentic-OS/Token-Less  
**RPM Source**: https://code.alibaba-inc.com/alinux/tokenless  
**System Requirements**: Rust 1.70+, Linux (Alinux 4 recommended)

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

**Token-Less** is an LLM token optimization toolkit that significantly reduces token consumption through **Schema/Response Compression** and **Command Rewriting** strategies.

### 1.1 Core Value Proposition

| Feature | Savings | Description |
|---------|---------|-------------|
| Schema Compression | ~57% | Compresses OpenAI Function Calling tool definitions |
| Response Compression | ~26–78% | Compresses API/tool responses (varies by content) |
| Command Rewriting | 60–90% | Filters CLI command output via RTK |

### 1.2 Supported Integrations

| Integration | Command Rewriting | Response Compression | Schema Compression |
|-------------|-------------------|---------------------|-------------------|
| OpenClaw Plugin | ✅ | ✅ | ⏳ (Limited by OpenClaw hook system) |
| Copilot Shell Hook | ✅ | ✅ | ⏳ (Waiting for protocol extension) |

### 1.3 Architecture Overview

```
Token-Less/
├── crates/tokenless-schema/   # Core library: SchemaCompressor + ResponseCompressor
├── crates/tokenless-cli/      # CLI binary: tokenless command
├── openclaw/                  # OpenClaw plugin (TypeScript)
├── hooks/copilot-shell/       # Copilot Shell Hooks
├── third_party/rtk/           # RTK submodule (command rewriting engine)
├── Makefile                   # Unified build system
├── scripts/install.sh         # One-step installation script
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
| R1 | String Truncation | Length > 512 bytes | Truncate at UTF-8 boundary, append `… (truncated)` | 512 bytes |
| R2 | Array Truncation | Elements > 16 | Keep first 16, append `<... N more items truncated>` | 16 elements |
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

---

## 3. System Requirements

| Dependency | Version | Purpose | Required |
|------------|---------|---------|----------|
| Rust | >= 1.70 (stable) | Compile tokenless and rtk | Build time only |
| Git | Any | Submodule management | Build time only |
| jq | Any | Hook script JSON processing | Yes |
| rtk | >= 0.28.0 | Command rewriting | Optional |
| tokenless | >= 0.1.0 | Schema/Response compression | Optional |

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
2. **Hook Scripts**: Installed to `/usr/share/tokenless/hooks/copilot-shell/`
3. **OpenClaw Plugin**: Auto-detected and configured (if OpenClaw is installed)
4. **Copilot Shell**: Auto-detected and configured (if Copilot Shell is installed)

**Verify RPM Installation**:
```bash
# Check binaries
which tokenless
# Output: /usr/bin/tokenless

tokenless --version

# Check hook scripts (RPM installation path)
ls -la /usr/share/tokenless/hooks/copilot-shell/

# Check OpenClaw plugin configuration
cat ~/.openclaw/openclaw.json | jq '.plugins.allow'
```

### 4.2 Method 2: One-Click Source Installation

```bash
# Clone repository (including submodules)
git clone --recursive https://code.alibaba-inc.com/Agentic-OS/Token-Less
cd Token-Less

# Full installation: build + install binaries + deploy OpenClaw plugin + Copilot Shell Hook
make setup
```

### 4.3 Method 3: Installation Script

```bash
# Auto-detect installation source and configure
./scripts/install.sh

# Force source installation
./scripts/install.sh --source

# Manual configuration after RPM installation
./scripts/install.sh --install

# Uninstall cleanup
./scripts/install.sh --uninstall
```

### 4.4 Method 4: Step-by-Step Installation

#### 4.4.1 Build

```bash
# Build tokenless + rtk (release mode)
make build

# Build tokenless only
make build-tokenless

# Build rtk only
make build-rtk
```

#### 4.4.2 Install Binaries

```bash
# Install to /usr/share/tokenless/bin (default)
make install

# Custom installation path
make install INSTALL_DIR=/usr/local/bin
```

#### 4.4.3 Deploy OpenClaw Plugin

```bash
# Using Makefile
make openclaw-install

# Custom plugin path
make openclaw-install OPENCLAW_DIR=/usr/share/tokenless/openclaw

# Manual installation
cp -r openclaw/ /usr/share/tokenless/openclaw/
```

#### 4.4.4 Deploy Copilot Shell Hook

```bash
# Using Makefile
make copilot-shell-install

# Manual installation
mkdir -p /usr/share/tokenless/hooks/copilot-shell
cp hooks/copilot-shell/tokenless-*.sh /usr/share/tokenless/hooks/copilot-shell/
chmod +x /usr/share/tokenless/hooks/copilot-shell/tokenless-*.sh
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

### 5.2 Post-Installation Auto-Configuration (RPM)

After RPM installation, the installation script automatically detects and configures installed platforms.

#### 5.2.1 Auto-Detected Platforms

| Platform | Detection Condition | Auto-Configuration |
|----------|---------------------|-------------------|
| OpenClaw | `~/.openclaw/openclaw.json` exists | Plugin deployment + plugins.allow configuration |
| Copilot Shell | `~/.copilot-shell/settings.json` or `~/.qwen-code/settings.json` exists | Hook script deployment + settings.json configuration |

#### 5.2.2 Manual Configuration Trigger

If reconfiguration is needed after RPM installation, run:

```bash
# Use system path configuration
/usr/share/tokenless/scripts/install.sh --install
```

#### 5.2.3 Verify Auto-Configuration

```bash
# Check OpenClaw plugin configuration
cat ~/.openclaw/openclaw.json | jq '.plugins.allow'
# Should contain "tokenless-openclaw"

# Check Copilot Shell Hook configuration
cat ~/.copilot-shell/settings.json | jq '.hooks | keys'
# Should contain PreToolUse, PostToolUse, BeforeModel

# Check hook scripts
ls -la /usr/share/tokenless/hooks/copilot-shell/
```

### 5.3 Copilot Shell Configuration

#### 5.3.1 Hook Script Locations

Hook script locations depend on the installation method:

| Installation Method | Hook Script Location |
|---------------------|---------------------|
| RPM Installation | `/usr/share/tokenless/hooks/copilot-shell/` |
| Source Installation | `/usr/share/tokenless/hooks/copilot-shell/` |

| Script | Function | Hook Event |
|--------|----------|------------|
| `tokenless-rewrite.sh` | Command rewriting | PreToolUse |
| `tokenless-compress-response.sh` | Response compression | PostToolUse |
| `tokenless-compress-schema.sh` | Schema compression | BeforeModel |

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
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-rewrite.sh",
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
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-compress-response.sh",
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
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-compress-schema.sh",
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
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-rewrite.sh",
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
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-compress-response.sh",
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
            "command": "/usr/share/tokenless/hooks/copilot-shell/tokenless-compress-schema.sh",
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
  → Calls tokenless compress-response 
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
  "verbose": false
}
```

| Option | Default | Description |
|--------|---------|-------------|
| `rtk_enabled` | `true` | Enable RTK command rewriting |
| `schema_compression_enabled` | `true` | Enable tool schema compression |
| `response_compression_enabled` | `true` | Enable tool response compression |
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

RESULT=$(echo "$INPUT" | bash /root/.copilot-shell/hooks/tokenless/tokenless-compress-response.sh 2>/dev/null)

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
echo '{"tool_input":{"command":"cargo test"}}' | bash hooks/copilot-shell/tokenless-rewrite.sh

# Test response compression (source directory)
echo '{"tool_name":"Shell","tool_response":"{\"stdout\":\"lots of verbose output here...\"}"}' | bash hooks/copilot-shell/tokenless-compress-response.sh

# Test schema compression (source directory)
echo '{"llm_request":{"tools":[{"name":"test","description":"A test tool","parameters":{}}]}}' | bash hooks/copilot-shell/tokenless-compress-schema.sh

# Test installed hook (RPM installation)
echo '{"tool_input":{"command":"cargo test"}}' | bash /usr/share/tokenless/hooks/copilot-shell/tokenless-rewrite.sh
```

### 6.2 CLI Testing

```bash
# Create test file
echo '{"status":"success","data":{"items":[1,2,3]},"debug":{"id":"abc"}}' > test.json

# Compress response
tokenless compress-response -f test.json

# Compress schema
echo '[{"name":"Shell","description":"Run shell commands","parameters":{"type":"object"}}]' | tokenless compress-schema
```

### 6.3 Verify Installation

```bash
# Check binaries
which tokenless
which rtk

# Check versions
tokenless --version
rtk --version

# Check hook scripts (RPM installation)
ls -la /usr/share/tokenless/hooks/copilot-shell/

# Check hook scripts (Source installation)
ls -la /usr/share/tokenless/hooks/copilot-shell/
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

### 7.2 OpenClaw Plugin

| Problem | Solution |
|---------|----------|
| Plugin not loaded | Check plugin path: `~/.openclaw/plugins/tokenless-openclaw/` |
| RTK not working | Ensure `rtk` is in `$PATH`, check `rtk_enabled` configuration |
| Compression not working | Check `response_compression_enabled` configuration |
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
| `make build-tokenless` | Build tokenless only |
| `make build-rtk` | Build rtk only |
| `make install` | Install binaries to INSTALL_DIR |
| `make test` | Run tests |
| `make lint` | Run clippy checks |
| `make fmt` | Format code |
| `make clean` | Clean build artifacts |
| `make openclaw-install` | Install OpenClaw plugin |
| `make openclaw-uninstall` | Uninstall OpenClaw plugin |
| `make copilot-shell-install` | Install Copilot Shell Hook |
| `make copilot-shell-uninstall` | Uninstall Copilot Shell Hook |
| `make setup` | Full installation: build + install + plugin deployment |

### 8.2 Key File Paths

| Purpose | File Path |
|---------|-----------|
| Core compression algorithm | `crates/tokenless-schema/src/response_compressor.rs` |
| Schema compression | `crates/tokenless-schema/src/schema_compressor.rs` |
| CLI subcommand | `crates/tokenless-cli/src/main.rs` |
| OpenClaw plugin | `openclaw/index.ts` |
| Copilot Hook | `hooks/copilot-shell/tokenless-*.sh` |
| Integration tests | `crates/tokenless-schema/tests/integration_test.rs` |

### 8.3 Fail-Open Design

All integration paths use **fail-open** strategy:

- **OpenClaw Plugin**: try-catch returns null → original result passes through
- **Copilot Shell Hook**: Any failure point exits with `exit 0` and no output → original result passes through
- **CLI**: Errors output to stderr, caller checks exit code to decide fallback

### 8.4 Default Configuration

| Parameter | Default | Builder Method |
|-----------|---------|----------------|
| `truncate_strings_at` | 512 | `with_truncate_strings_at(len)` |
| `truncate_arrays_at` | 16 | `with_truncate_arrays_at(len)` |
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
**Document Version**: 1.1
**Last Updated**: 2026-04-13
