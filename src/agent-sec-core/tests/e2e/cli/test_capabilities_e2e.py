"""E2E tests for ``agent-sec-cli capabilities``."""

import json
import os

from cli.conftest import run_cli


class _EnvPatch:
    def __init__(self, **values: str) -> None:
        self.values = values
        self.previous: dict[str, str | None] = {}

    def __enter__(self) -> None:
        for name, value in self.values.items():
            self.previous[name] = os.environ.get(name)
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
    assert payload[0]["env"]["CODE_SCANNER_HOOK_ENABLED"]["raw"] == "false"


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
    assert payload[0]["config"] == {}
    assert payload[0]["config_path"] is None
    assert payload[0]["env"]["PROMPT_SCANNER_HOOK_ENABLED"]["raw"] == "false"


def test_capabilities_rejects_noncanonical_capability_name() -> None:
    result = run_cli("capabilities", "--capability", "scan-code")

    assert result.returncode == 1
    assert "unknown capability" in result.stderr
