"""Telemetry path and component metadata."""

import os
from pathlib import Path

from agent_sec_cli import __version__

COMPONENT_NAME = "agent-sec-core"
COMPONENT_AGENT_NAME = ""
DEFAULT_TELEMETRY_LOG_PATH = "/var/log/anolisa/sls/ops/agent-sec-core.jsonl"
TELEMETRY_LOG_PATH_ENV = "AGENT_SEC_TELEMETRY_LOG_PATH"


def get_telemetry_log_path() -> Path:
    """Return the configured Agentic OS telemetry JSONL path."""
    override = os.environ.get(TELEMETRY_LOG_PATH_ENV)
    if override:
        return Path(override).expanduser()
    return Path(DEFAULT_TELEMETRY_LOG_PATH)


def telemetry_log_path_exists() -> bool:
    """Return whether the configured telemetry JSONL file exists."""
    return get_telemetry_log_path().is_file()


def get_component_fields() -> dict[str, str]:
    """Return fixed Agentic OS component fields for telemetry records."""
    return {
        "component.name": COMPONENT_NAME,
        "component.version": __version__,
        "component.agent_name": COMPONENT_AGENT_NAME,
    }
