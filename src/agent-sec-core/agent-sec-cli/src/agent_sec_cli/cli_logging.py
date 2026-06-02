"""Diagnostic JSONL logging for the agent-sec-cli process."""

import logging
import os
import traceback as traceback_module
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from agent_sec_cli.correlation_context import (
    get_current_trace_context,
    get_invocation_id,
)
from agent_sec_cli.security_events.config import get_stream_log_path
from agent_sec_cli.security_events.writer import JsonlEventWriter

_LOGGER_NAME = "agent_sec_cli"
_ENV_LOG_LEVEL = "AGENT_SEC_CLI_LOG_LEVEL"
_ENV_LOG_DISABLED = "AGENT_SEC_CLI_LOG_DISABLED"
_TRUE_VALUES = frozenset({"1", "true", "yes", "on"})
_LEVELS = {
    "debug": logging.DEBUG,
    "info": logging.INFO,
    "warning": logging.WARNING,
    "error": logging.ERROR,
    "critical": logging.CRITICAL,
}
_CORRELATION_FIELDS = (
    "trace_id",
    "session_id",
    "run_id",
    "call_id",
    "tool_call_id",
)

# Diagnostic stream is sized smaller than the business streams: default
# WARNING volume is low, and DEBUG bursts roll over quickly.
CLI_LOG_MAX_BYTES = 10 * 1024 * 1024
CLI_LOG_BACKUP_COUNT = 5

_SETUP_DONE = False


@dataclass(frozen=True)
class CliLoggingConfig:
    """Resolved CLI diagnostic logging configuration."""

    enabled: bool
    level: int
    log_file: Path | None


def _utc_now_iso() -> str:
    return (
        datetime.now(timezone.utc)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z")
    )


def _is_truthy(value: str | None) -> bool:
    return value is not None and value.strip().lower() in _TRUE_VALUES


def _resolve_level(value: str | None) -> tuple[bool, int]:
    if value is None:
        return True, logging.WARNING

    normalized = value.strip().lower()
    if normalized == "off":
        return False, logging.WARNING
    return True, _LEVELS.get(normalized, logging.WARNING)


def _resolve_log_path() -> Path | None:
    """Resolve `cli.jsonl` through the shared data-dir helper.

    `get_stream_log_path("cli")` already routes through `_resolve_data_dir()`,
    which `mkdir`s the parent at mode 0o700 and `chmod`s it on every call.
    All three streams (security-events, observability, cli) share this path
    resolution and the same access-control guarantees.
    """
    try:
        return Path(get_stream_log_path("cli")).expanduser()
    except Exception:  # noqa: BLE001 - diagnostic logging is best-effort
        return None


def resolve_cli_logging_config() -> CliLoggingConfig:
    """Resolve CLI diagnostic logging settings from environment variables."""
    level_enabled, level = _resolve_level(os.environ.get(_ENV_LOG_LEVEL))
    if _is_truthy(os.environ.get(_ENV_LOG_DISABLED)) or not level_enabled:
        return CliLoggingConfig(enabled=False, level=level, log_file=None)

    log_file = _resolve_log_path()
    if log_file is None:
        return CliLoggingConfig(enabled=False, level=level, log_file=None)
    return CliLoggingConfig(enabled=True, level=level, log_file=log_file)


class JsonlCliLogHandler(logging.Handler):
    """Convert Python log records into CLI diagnostic JSONL records."""

    def __init__(self, path: str | Path) -> None:
        super().__init__()
        self._path = Path(path).expanduser()
        self._writer = JsonlEventWriter(
            self._path,
            max_bytes=CLI_LOG_MAX_BYTES,
            backup_count=CLI_LOG_BACKUP_COUNT,
        )

    def emit(self, record: logging.LogRecord) -> None:
        """Write one logging record. All handler failures are swallowed."""
        try:
            self._writer.write(self._record_to_payload(record))
        except Exception:  # noqa: BLE001
            pass

    def _record_to_payload(self, record: logging.LogRecord) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "timestamp": _utc_now_iso(),
            "level": record.levelname,
            "logger": record.name,
            "message": record.getMessage(),
            "function": record.funcName,
            "invocation_id": get_invocation_id(),
        }

        # Correlation slots: process trace context first, then per-record
        # overrides via extra={"trace_id": ...}. Only stamp when non-empty —
        # do not fabricate UUIDs to fill empty slots.
        trace_ctx = get_current_trace_context()
        if trace_ctx is not None:
            for field_name in _CORRELATION_FIELDS:
                value = getattr(trace_ctx, field_name, None)
                if value:
                    payload[field_name] = value
        for field_name in _CORRELATION_FIELDS:
            value = getattr(record, field_name, None)
            if value:
                payload[field_name] = value

        # Free-form payload slot. Caller owns shape (string or dict); no
        # whitelist, no schema validation. Call-site discipline is the only
        # guard against leaking large or sensitive content here.
        data = getattr(record, "data", None)
        if data is not None:
            payload["data"] = data

        if record.exc_info:
            exception = record.exc_info[1]
            if exception is not None:
                payload["error_type"] = type(exception).__name__
                payload["exception"] = str(exception)
            if record.levelno >= logging.ERROR:
                payload["traceback"] = "".join(
                    traceback_module.format_exception(*record.exc_info)
                )
        return payload


def setup_cli_logging() -> None:
    """Idempotently configure diagnostic logging for the agent_sec_cli tree.

    Failures (config resolution or handler construction) leave the logger
    unattached but still mark setup as done — diagnostic logging never retries
    mid-process. The first call is the only call that has any effect.
    """
    global _SETUP_DONE  # noqa: PLW0603
    if _SETUP_DONE:
        return
    logger = logging.getLogger(_LOGGER_NAME)
    logger.propagate = False
    try:
        config = resolve_cli_logging_config()
        if config.enabled and config.log_file is not None:
            handler = JsonlCliLogHandler(config.log_file)
            handler.setLevel(config.level)
            logger.addHandler(handler)
            logger.setLevel(config.level)
    except Exception:  # noqa: BLE001 - diagnostic logging setup is best-effort
        pass
    finally:
        _SETUP_DONE = True


def _reset_cli_logging_for_tests() -> None:
    """Reset module logging state for in-process unit tests."""
    global _SETUP_DONE  # noqa: PLW0603
    logger = logging.getLogger(_LOGGER_NAME)
    for handler in list(logger.handlers):
        if isinstance(handler, JsonlCliLogHandler):
            logger.removeHandler(handler)
            handler.close()
    logger.propagate = True
    _SETUP_DONE = False
