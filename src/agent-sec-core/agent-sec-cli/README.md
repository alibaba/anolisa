# Agent Security Core CLI

[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Python](https://img.shields.io/badge/Python-3.6+-blue.svg)](https://www.python.org/downloads/)
[![Version](https://img.shields.io/badge/version-0.0.1-green.svg)](CHANGELOG.md)

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

---

## Installation

### From Source

```bash
# Clone the repository
git clone https://github.com/alibaba/anolisa.git
cd anolisa/src/agent-sec-core/agent-sec-cli

# Install in development mode
pip install -e .

# Or build and install the wheel
pip install build
python -m build
pip install dist/agent_sec_cli-0.0.1-py3-none-any.whl
```

### From RPM Package

```bash
# Build RPM (from parent directory)
cd ..
make rpm

# Install RPM
sudo rpm -i agent-sec-core-0.0.1-1.el8.x86_64.rpm
```

### Dependencies

**Required:**
- Python 3.6+
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
agent-sec-cli harden --mode scan
agent-sec-cli harden --mode reinforce --config agentos_baseline
agent-sec-cli harden --mode dry-run

# Skill integrity verification
agent-sec-cli verify
agent-sec-cli verify --skill /path/to/skill

# Security event summary
agent-sec-cli summary --hours 24 --format text
agent-sec-cli summary --hours 72 --format json
```

### Python API

```python
from agent_sec_cli.security_middleware import invoke

# System hardening
result = invoke("harden", mode="scan")
print(result.stdout)

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
# Clone and install with dev dependencies
pip install -e ".[dev]"

# Run tests
pytest tests/

# Format code
black src/
isort src/
```

### Running Tests

```bash
# Unit tests
pytest tests/unit-test/

# Integration tests
pytest tests/integration-test/

# All tests with coverage
pytest --cov=agent_sec_cli tests/
```

### Building from Source

```bash
# Install build dependencies
pip install build wheel setuptools

# Build wheel and sdist
python -m build

# Output:
# dist/
#   ├── agent_sec_cli-0.0.1-py3-none-any.whl
#   └── agent-sec-cli-0.0.1.tar.gz
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

**Issue:** `ModuleNotFoundError: No module named 'agent_sec_cli'`
- **Solution:** Ensure the package is installed: `pip install -e .`

**Issue:** `Verification failed: No trusted keys found`
- **Solution:** Add trusted GPG keys to `asset_verify/trusted-keys/`

**Issue:** `Permission denied` errors during hardening
- **Solution:** Run with sudo: `sudo agent-sec-cli harden --mode reinforce`

### Debug Mode

Enable verbose output:

```bash
python -m agent_sec_cli.cli harden --mode scan 2>&1 | tee debug.log
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
