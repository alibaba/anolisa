# Agent Security Core CLI

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Python](https://img.shields.io/badge/Python-3.10+-blue.svg)](https://www.python.org/downloads/)
[![Version](https://img.shields.io/badge/version-0.3.0-green.svg)](CHANGELOG.md)

**Agent Security Core CLI** is a comprehensive security toolkit for AI Agents, providing system hardening, sandbox isolation, asset integrity verification, and security event tracking.

---

## Features

### 🔒 System Hardening
- Security baseline scanning and assessment
- Automated reinforcement with configurable baselines
- Dry-run mode for safe testing
- Integration with LoongShield security framework

### 🏖️ Sandbox Isolation
- Command classification and risk assessment
- Dynamic sandbox policy generation
- Integration with bubblewrap for process isolation
- Fine-grained resource and access control

### ✅ Asset Integrity Verification
- GPG-signed skill manifests
- SHA-256 hash verification for all files
- Trusted key management
- Batch verification for multiple skills

### 📊 Security Event Tracking
- JSONL-based event logging
- Thread-safe logging with rotation detection
- Time-range based event aggregation
- Multiple output formats (text, JSON)

### 📈 Observability Record Ingestion
- Typed agent hook record validation
- Independent `observability.jsonl` stream
- Forward-compatible unknown field and metric filtering
- JSON Schema output for producers

---

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/alibaba/anolisa.git
cd anolisa/src/agent-sec-core/agent-sec-cli

# Install in development mode (uv manages .venv automatically)
uv sync

# Or build the wheel
uv run maturin build --release
```

### From RPM Package

```bash
# Build RPM (from parent directory)
cd ..
make rpm

# Install RPM
sudo rpm -i agent-sec-core-0.3.0-1.el8.x86_64.rpm
```

### Dependencies

**Required:**
- Python 3.10+
- [uv](https://docs.astral.sh/uv/) (Python package manager)
- GnuPG 2.0+

**Optional:**
- `pgpy >= 0.5` - Pure Python PGP implementation (faster verification)
- `bubblewrap` - Sandbox isolation backend

---

## Usage

### Command-Line Interface

After installation, use the `agent-sec-cli` command:

```bash
# System hardening
agent-sec-cli harden --scan --config agentos_baseline
agent-sec-cli harden --reinforce --config agentos_baseline
agent-sec-cli harden --reinforce --dry-run --config agentos_baseline

# Skill integrity verification
agent-sec-cli verify
agent-sec-cli verify --skill /path/to/skill

# Security event summary
agent-sec-cli summary --hours 24 --format text
agent-sec-cli summary --hours 72 --format json

# Observability record ingestion
agent-sec-cli observability record --format json --stdin < record.json
agent-sec-cli observability schema
```

### Observability Records

`agent-sec-cli observability record` accepts one JSON object from stdin and writes
validated hook telemetry to the independent `observability.jsonl` stream.

Required wire fields:

- `hook`
- `observedAt` as a timezone-aware timestamp
- `metadata.sessionId`
- `metadata.runId`
- `metrics`

Hook-specific metadata:

- `metadata.callId` is optional on model and tool call records.
- `metadata.toolCallId` is required on `before_tool_call` and `after_tool_call`.

Unknown top-level fields, metadata fields, and metric keys are ignored for
forward compatibility. A record is rejected when no supported metric remains
after filtering. The command is silent on success and exits non-zero if parsing,
validation, or persistence fails.

Current supported hooks:

- `before_agent_run`
- `before_llm_call`
- `after_llm_call`
- `before_tool_call`
- `after_tool_call`
- `after_agent_run`

### Python API

```python
from agent_sec_cli.security_middleware import invoke

# System hardening
result = invoke("harden", args=["--scan", "--config", "agentos_baseline"])
print(result.success)

# Verify a specific skill
result = invoke("verify", skill="/path/to/skill")
if result.success:
    print("Verification passed!")
else:
    print(f"Verification failed: {result.error}")

# Get security event summary
result = invoke("summary", hours=24, format="json")
print(result.stdout)
```

---

## Architecture

```
agent_sec_cli/
├── cli.py                      # Unified CLI entry point
├── asset_verify/               # Integrity verification
│   ├── verifier.py            # Main verification logic
│   ├── errors.py              # Custom exception types
│   ├── config.conf            # Configuration file
│   └── trusted-keys/          # Trusted GPG public keys
├── sandbox/                    # Sandbox policy generation
│   ├── sandbox_policy.py      # Policy generation
│   ├── classify_command.py    # Command classification
│   └── rules.py               # Security rules
├── security_events/            # Event logging
│   ├── writer.py              # JSONL event writer
│   ├── schema.py              # Event schema definitions
│   └── config.py              # Logging configuration
└── security_middleware/        # Unified middleware layer
    ├── __init__.py            # Main entry point (invoke)
    ├── router.py              # Action routing
    ├── lifecycle.py           # Pre/post hooks
    ├── context.py             # Request context
    ├── result.py              # Result wrapper
    └── backends/              # Backend implementations
        ├── hardening.py       # System hardening backend
        ├── sandbox.py         # Sandbox backend
        ├── asset_verify.py    # Verification backend
        ├── summary.py         # Event summary backend
        └── intent.py          # Intent analysis (future)
```

---

## Development

### Setup Development Environment

```bash
# Clone and install all dependencies (dev included by default)
cd agent-sec-cli && uv sync

# Run tests (from agent-sec-core directory)
make test-python

# Format code
uv run black src/
uv run isort src/
```

### Running Tests

```bash
# Unit tests
uv run --project agent-sec-cli pytest tests/unit-test/

# Integration tests
uv run --project agent-sec-cli pytest tests/integration-test/

# All tests with coverage
uv run --project agent-sec-cli pytest --cov=agent_sec_cli tests/
```

### Building from Source

```bash
# Build wheel (maturin + Rust extension)
uv run maturin build --release

# Output:
# target/wheels/
#   └── agent_sec_cli-0.3.0-cp312-cp312-linux_x86_64.whl
```

---

## Configuration

### Asset Verification

Edit `asset_verify/config.conf`:

```ini
skills_dir = [
    /usr/share/anolisa/skills
    /opt/custom-skills
]
```

### Security Events

Event logging configuration is managed in `security_events/config.py`:

```python
LOG_FILE = "/var/log/agent-sec/security-events.jsonl"
MAX_FILE_SIZE = 10 * 1024 * 1024  # 10 MB
ROTATION_COUNT = 5
```

### Observability

Observability records use the same data directory resolver as security events,
but write to a separate stream:

- default system path: `/var/log/agent-sec/observability.jsonl`
- user fallback: `~/.agent-sec-core/observability.jsonl`
- test/dev override: `AGENT_SEC_DATA_DIR=/path/to/dir`

The observability stream uses its own JSONL file, lock file, rotation limit, and
backup count; it does not write to `security-events.jsonl`.

---

## Security

### Signing Skills

```bash
# Sign a single skill
sign-skill.sh /path/to/skill

# Sign all skills in batch
sign-skill.sh --batch /usr/share/anolisa/skills --force
```

### Verifying Skills

```bash
# Verify all configured skills
agent-sec-cli verify

# Verify with detailed output
python -m agent_sec_cli.asset_verify.verifier --skill /path/to/skill
```

---

## Troubleshooting

### Common Issues

**Issue:** `Verification failed: No trusted keys found`
- **Solution:** Add trusted GPG keys to `asset_verify/trusted-keys/`

**Issue:** `Permission denied` errors during hardening
- **Solution:** Run with sudo: `sudo agent-sec-cli harden --reinforce --config agentos_baseline`

### Debug Mode

Enable verbose output:

```bash
python -m agent_sec_cli.cli harden --scan --config agentos_baseline 2>&1 | tee debug.log
```

---

## Extending with dev-tools

The `dev-tools/` directory contains developer guides and skills for adding new security capabilities to agent-sec-cli.

### Quick Start: Add a New Security Command

Follow the step-by-step guide in [dev-tools/SKILL.md](dev-tools/SKILL.md) to:

1. **Add a CLI subcommand** - Define new command-line interface
2. **Register a router** - Map action names to backend modules
3. **Create a backend** - Implement security logic (Python or Rust)
4. **Integrate event logging** - Automatic security event tracking

### Architecture Overview

```
New Security Capability
├── CLI Layer (cli.py)
│   └── Add subcommand with argparse
├── Router Layer (router.py)
│   └── Register action → backend mapping
├── Backend Layer (backends/)
│   ├── Python backend (.py) — delegates to Python module
│   └── Rust backend (.py) — delegates to Rust PyO3 extension
└── Event Logging (security_events/)
    └── Automatic JSONL event recording
```

### Example: Adding a New Backend

**Python Backend:**
```
Use backend-skill in folder dev-tools to create a new python backend called my_scanner with module_path agent_sec_cli.my_scanner.analyzer
```

**Rust Backend:**
```
Use backend-skill in folder dev-tools to create a new rust backend called crypto_verify
```

### Development Resources

| Resource | Location | Purpose |
|----------|----------|---------|
| Extension Guide | `dev-tools/backend-skill/SKILL.md` | Step-by-step tutorial for Rust & Python backends |
| Backend Templates | `dev-tools/backend-skill/templates/` | Python and Rust backend templates |
| Backend Examples | `src/agent_sec_cli/security_middleware/backends/` | Reference implementations |
| CLI Structure | `src/agent_sec_cli/cli.py` | Subcommand patterns |
| Event Schema | `src/agent_sec_cli/security_events/schema.py` | Logging format |

---

## Contributing

We welcome contributions! Please see our [Contributing Guide](https://github.com/alibaba/anolisa/blob/main/CONTRIBUTING.md) for details.

1. Fork the repository
2. Create a feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

---

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

---

## Acknowledgments

- Part of the [ANOLISA](https://github.com/alibaba/anolisa) project
- Developed by Alibaba Cloud and the open-source community
- Inspired by security best practices for AI Agent platforms

---

## Support

- **Issues:** [GitHub Issues](https://github.com/alibaba/anolisa/issues)
- **Discussions:** [GitHub Discussions](https://github.com/alibaba/anolisa/discussions)
- **Email:** [anolisa@lists.openanolis.cn](mailto:anolisa@lists.openanolis.cn)
