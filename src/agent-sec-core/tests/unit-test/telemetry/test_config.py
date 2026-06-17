"""Unit tests for telemetry configuration."""

from pathlib import Path

from agent_sec_cli import __version__
from agent_sec_cli.telemetry.config import (
    DEFAULT_TELEMETRY_LOG_PATH,
    TELEMETRY_LOG_PATH_ENV,
    get_component_fields,
    get_telemetry_log_path,
    telemetry_log_path_exists,
)


def test_default_telemetry_log_path_is_agentic_os_component_file(
    monkeypatch,
) -> None:
    monkeypatch.delenv(TELEMETRY_LOG_PATH_ENV, raising=False)

    assert get_telemetry_log_path() == Path(DEFAULT_TELEMETRY_LOG_PATH)


def test_telemetry_log_path_env_override(monkeypatch, tmp_path: Path) -> None:
    path = tmp_path / "agent-sec-core.jsonl"
    monkeypatch.setenv(TELEMETRY_LOG_PATH_ENV, str(path))

    assert get_telemetry_log_path() == path


def test_telemetry_log_path_exists_only_for_existing_file(
    monkeypatch, tmp_path: Path
) -> None:
    path = tmp_path / "agent-sec-core.jsonl"
    monkeypatch.setenv(TELEMETRY_LOG_PATH_ENV, str(path))
    assert telemetry_log_path_exists() is False

    path.write_text("", encoding="utf-8")
    assert telemetry_log_path_exists() is True


def test_component_fields_are_fixed() -> None:
    assert get_component_fields() == {
        "component.name": "agent-sec-core",
        "component.version": __version__,
        "component.agent_name": "",
    }
