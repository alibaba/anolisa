"""E2E tests for ``agent-sec-cli capabilities``."""

import json
import os
import re
from functools import lru_cache

import pytest
from cli.conftest import run_cli

_ANSI_ESCAPE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")

_CAPABILITY_ENV_NAMES = (
    "CODE_SCANNER_HOOK_ENABLED",
    "CODE_SCANNER_MODE",
    "CODE_SCANNER_TIMEOUT",
    "OBSERVABILITY_HOOK_ENABLED",
    "OBSERVABILITY_TIMEOUT",
    "PII_CHECKER_ENABLED",
    "PII_CHECKER_HOOK_ENABLED",
    "PII_CHECKER_INCLUDE_LOW_CONFIDENCE",
    "PII_CHECKER_MODE",
    "PII_CHECKER_TIMEOUT",
    "PROMPT_SCANNER_HOOK_ENABLED",
    "PROMPT_SCANNER_L2_MODEL",
    "PROMPT_SCANNER_MODE",
    "PROMPT_SCANNER_SCAN_MODE",
    "PROMPT_SCANNER_TIMEOUT",
    "SKILL_LEDGER_HOOK_ENABLED",
    "SKILL_LEDGER_MODE",
    "SKILL_LEDGER_TIMEOUT",
)
_AGENTS = ("qoder", "qwen", "codex", "cosh", "openclaw", "hermes")
_CAPABILITIES = (
    "code-scan",
    "prompt-scan",
    "pii-check",
    "skill-ledger",
    "observability",
)


class _EnvPatch:
    """Override env vars for a block; ``None`` unsets one for its duration."""

    def __init__(self, **values: str | None) -> None:
        self.values = values
        self.previous: dict[str, str | None] = {}

    def __enter__(self) -> None:
        names = set(_CAPABILITY_ENV_NAMES) | self.values.keys()
        for name in names:
            self.previous[name] = os.environ.get(name)
            value = self.values.get(name)
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value

    def __exit__(self, *_args: object) -> None:
        for name, value in self.previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def test_capabilities_json_uses_cli_process_environment() -> None:
    with _EnvPatch(CODE_SCANNER_HOOK_ENABLED="false"):
        result = run_cli(
            "capabilities",
            "--agent",
            "qoder",
            "--capability",
            "code-scan",
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload[0]["enabled"] == "disabled"
    assert payload[0]["mode"] == "observe"
    assert payload[0]["scan_mode"] == "-"
    assert payload[0]["timeout"] == "10"
    assert "hooks" not in payload[0]
    assert "source" not in payload[0]
    assert "raw" not in payload[0]["env"]["CODE_SCANNER_HOOK_ENABLED"]
    assert payload[0]["env"]["CODE_SCANNER_HOOK_ENABLED"]["effective"] is False


def test_capabilities_json_reads_new_hook_enabled_environment() -> None:
    with _EnvPatch(PROMPT_SCANNER_HOOK_ENABLED="false"):
        result = run_cli(
            "capabilities",
            "--agent",
            "codex",
            "--capability",
            "prompt-scan",
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload[0]["enabled"] == "disabled"
    assert payload[0]["mode"] == "observe"
    assert payload[0]["scan_mode"] == "standard"
    assert "config" not in payload[0]
    assert "config_path" not in payload[0]
    assert "raw" not in payload[0]["env"]["PROMPT_SCANNER_HOOK_ENABLED"]
    assert payload[0]["env"]["PROMPT_SCANNER_HOOK_ENABLED"]["effective"] is False


def test_capabilities_json_reads_observability_timeout_environment() -> None:
    with _EnvPatch(OBSERVABILITY_TIMEOUT="3"):
        result = run_cli(
            "capabilities",
            "--agent",
            "openclaw",
            "--capability",
            "observability",
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert payload[0]["timeout"] == "3"
    assert payload[0]["env"]["OBSERVABILITY_TIMEOUT"] == {
        "effective": "3",
        "default": "5",
    }


@lru_cache(maxsize=1)
def _engine_l2_default() -> str:
    """L2 default the CLI reports, or `""` when the extension is not built.

    Probed through the CLI instead of importing the extension here: the
    installed-artifact runs drive a binary whose interpreter is not the one
    running pytest and cannot import the package at all, so an in-process
    probe would report no engine while the CLI reports a real default.
    """
    with _EnvPatch(PROMPT_SCANNER_L2_MODEL=None):
        result = run_cli(
            "capabilities",
            "--capability",
            "prompt-scan",
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    default = json.loads(result.stdout)[0]["env"]["PROMPT_SCANNER_L2_MODEL"]["default"]
    assert isinstance(default, str), default
    return default


def test_capabilities_json_reports_prompt_scanner_l2_model() -> None:
    model = "modelscope.cn/ANOLISA/Warden-Gen-0.6B-GGUF"
    with _EnvPatch(PROMPT_SCANNER_L2_MODEL=model):
        result = run_cli(
            "capabilities",
            "--capability",
            "prompt-scan",
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert len(payload) == 6
    for record in payload:
        assert record["env"]["PROMPT_SCANNER_L2_MODEL"] == {
            "effective": model,
            "default": _engine_l2_default(),
        }
        assert record["diagnostics"] == []


def test_capabilities_json_flags_unsupported_prompt_scanner_l2_model() -> None:
    model = "modelscope.cn/ANOLISA/Warden-Gen-0.6B-GGUF-3Domain:q4_K_M"
    with _EnvPatch(PROMPT_SCANNER_L2_MODEL=model):
        result = run_cli(
            "capabilities",
            "--agent",
            "qoder",
            "--capability",
            "prompt-scan",
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    # The value is always reported; only the check needs the engine, so without
    # the extension the same run must stay diagnostic-free.
    expected_diagnostics = (
        [
            "PROMPT_SCANNER_L2_MODEL is not a supported L2 backend; "
            "prompt scans will fail"
        ]
        if _engine_l2_default()
        else []
    )
    assert payload[0]["env"]["PROMPT_SCANNER_L2_MODEL"]["effective"] == model
    assert payload[0]["diagnostics"] == expected_diagnostics


def test_capabilities_rejects_noncanonical_capability_name() -> None:
    result = run_cli("capabilities", "--capability", "scan-code")

    assert result.returncode == 1
    assert "unknown capability" in result.stderr


def test_capabilities_json_lists_complete_default_matrix_in_stable_order() -> None:
    with _EnvPatch():
        result = run_cli("capabilities", "--output", "json")

    assert result.returncode == 0, result.stderr
    assert result.stderr == ""
    payload = json.loads(result.stdout)
    assert len(payload) == len(_AGENTS) * len(_CAPABILITIES)
    assert [(item["agent"], item["capability"]) for item in payload] == sorted(
        (agent, capability) for agent in _AGENTS for capability in _CAPABILITIES
    )
    assert all(item["enabled"] == "enabled" for item in payload)
    assert all(
        set(item)
        == {
            "agent",
            "capability",
            "enabled",
            "mode",
            "scan_mode",
            "timeout",
            "env",
            "diagnostics",
        }
        for item in payload
    )


def test_capabilities_default_table_groups_every_agent_and_capability() -> None:
    with _EnvPatch():
        result = run_cli("capabilities")

    assert result.returncode == 0, result.stderr
    assert result.stderr == ""
    assert result.stdout.count("CAPABILITY") == len(_AGENTS)
    for agent in _AGENTS:
        assert result.stdout.count(f"[{agent}]") == 1
    for capability in _CAPABILITIES:
        assert result.stdout.count(capability) == len(_AGENTS)
    assert "HOOKS" not in result.stdout
    assert "SOURCE" not in result.stdout


@pytest.mark.parametrize("agent", _AGENTS)
@pytest.mark.parametrize("capability", _CAPABILITIES)
def test_capabilities_filters_every_supported_agent_capability_pair(
    agent: str, capability: str
) -> None:
    with _EnvPatch():
        result = run_cli(
            "capabilities",
            "--agent",
            agent,
            "--capability",
            capability,
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert len(payload) == 1
    assert payload[0]["agent"] == agent
    assert payload[0]["capability"] == capability


def test_capabilities_short_filters_trim_and_normalize_case() -> None:
    with _EnvPatch():
        result = run_cli(
            "capabilities",
            "-a",
            " QODER ",
            "-c",
            " PROMPT-SCAN ",
            "-o",
            "json",
        )

    assert result.returncode == 0, result.stderr
    payload = json.loads(result.stdout)
    assert [(item["agent"], item["capability"]) for item in payload] == [
        ("qoder", "prompt-scan")
    ]


@pytest.mark.parametrize(
    ("agent", "raw_mode", "expected_mode", "expected_diagnostics"),
    [
        ("qoder", "debug", "observe", []),
        ("qoder", "deny", "block", []),
        ("codex", "ask", "observe", ["CODE_SCANNER_MODE"]),
        ("cosh", "block", "ask", ["CODE_SCANNER_MODE"]),
    ],
)
def test_capabilities_applies_agent_specific_code_scanner_modes(
    agent: str,
    raw_mode: str,
    expected_mode: str,
    expected_diagnostics: list[str],
) -> None:
    with _EnvPatch(CODE_SCANNER_MODE=raw_mode):
        result = run_cli(
            "capabilities",
            "--agent",
            agent,
            "--capability",
            "code-scan",
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    record = json.loads(result.stdout)[0]
    assert record["mode"] == expected_mode
    assert bool(record["diagnostics"]) is bool(expected_diagnostics)
    for name in expected_diagnostics:
        assert any(name in diagnostic for diagnostic in record["diagnostics"])


@pytest.mark.parametrize(
    ("environment", "expected_enabled"),
    [
        ({"PII_CHECKER_ENABLED": "false"}, "disabled"),
        (
            {
                "PII_CHECKER_ENABLED": "false",
                "PII_CHECKER_HOOK_ENABLED": "true",
            },
            "enabled",
        ),
        (
            {
                "PII_CHECKER_ENABLED": "true",
                "PII_CHECKER_HOOK_ENABLED": "false",
            },
            "disabled",
        ),
    ],
)
def test_capabilities_qwen_pii_new_enabled_variable_takes_precedence(
    environment: dict[str, str], expected_enabled: str
) -> None:
    with _EnvPatch(**environment):
        result = run_cli(
            "capabilities",
            "--agent",
            "qwen",
            "--capability",
            "pii-check",
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)[0]["enabled"] == expected_enabled


@pytest.mark.parametrize(
    ("agent", "capability", "environment", "expected_timeout", "diagnostic"),
    [
        ("qwen", "pii-check", {"PII_CHECKER_TIMEOUT": "99"}, "8", None),
        ("qoder", "skill-ledger", {"SKILL_LEDGER_TIMEOUT": "0.05"}, "0.05", None),
        ("qoder", "code-scan", {"CODE_SCANNER_TIMEOUT": "0"}, "0", "fail open"),
        ("qoder", "code-scan", {"CODE_SCANNER_TIMEOUT": "1.5"}, "10", "invalid"),
        ("hermes", "observability", {"OBSERVABILITY_TIMEOUT": "999"}, "5", None),
        ("openclaw", "observability", {"OBSERVABILITY_TIMEOUT": "0"}, "5", "invalid"),
    ],
)
def test_capabilities_reports_runtime_compatible_timeout_semantics(
    agent: str,
    capability: str,
    environment: dict[str, str],
    expected_timeout: str,
    diagnostic: str | None,
) -> None:
    with _EnvPatch(**environment):
        result = run_cli(
            "capabilities",
            "--agent",
            agent,
            "--capability",
            capability,
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    record = json.loads(result.stdout)[0]
    assert record["timeout"] == expected_timeout
    if diagnostic is None:
        assert record["diagnostics"] == []
    else:
        assert any(diagnostic in item for item in record["diagnostics"])


def test_capabilities_invalid_environment_value_is_diagnosed_without_raw_leak() -> None:
    sensitive_value = "invalid-token-like-value"
    with _EnvPatch(PROMPT_SCANNER_MODE=sensitive_value):
        result = run_cli(
            "capabilities",
            "--agent",
            "qwen",
            "--capability",
            "prompt-scan",
            "--output",
            "json",
        )

    assert result.returncode == 0, result.stderr
    assert sensitive_value not in result.stdout
    assert sensitive_value not in result.stderr
    record = json.loads(result.stdout)[0]
    assert record["mode"] == "observe"
    assert record["env"]["PROMPT_SCANNER_MODE"] == {
        "effective": "observe",
        "default": "observe",
    }
    assert record["diagnostics"] == [
        "PROMPT_SCANNER_MODE has an invalid value; using 'observe'"
    ]


@pytest.mark.parametrize(
    ("arguments", "error"),
    [
        (("--agent", "unsupported"), "unknown agent"),
        (("--output", "yaml"), "--output must be one of"),
    ],
)
def test_capabilities_rejects_invalid_cli_values(
    arguments: tuple[str, str], error: str
) -> None:
    with _EnvPatch():
        result = run_cli("capabilities", *arguments)

    assert result.returncode == 1
    assert result.stdout == ""
    assert error in result.stderr


def test_capabilities_help_documents_filters_and_environment_scope() -> None:
    # Rich colorizes help output on CI, which splits option names across escape
    # sequences and rewraps prose at the detected width. Pin both so the literal
    # matches below describe the documented text rather than the rendering.
    with _EnvPatch(NO_COLOR="1", COLUMNS="200"):
        result = run_cli("capabilities", "--help")

    assert result.returncode == 0, result.stderr
    help_text = _ANSI_ESCAPE.sub("", result.stdout)
    assert "--agent" in help_text
    assert "--capability" in help_text
    assert "--output" in help_text
    assert "current CLI environment" in help_text
