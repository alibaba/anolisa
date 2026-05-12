"""JSONL persistence for observability records."""

from pathlib import Path

from agent_sec_cli.observability.schema import ObservabilityRecord
from agent_sec_cli.security_events.config import get_stream_log_path
from agent_sec_cli.security_events.writer import JsonlEventWriter

OBSERVABILITY_STREAM = "observability"
DEFAULT_OBSERVABILITY_MAX_BYTES = 256 * 1024 * 1024
DEFAULT_OBSERVABILITY_BACKUP_COUNT = 3

_writer: "ObservabilityJsonlWriter | None" = None


class ObservabilityJsonlWriter:
    """Append observability records to the independent observability JSONL stream."""

    def __init__(
        self,
        path: str | Path | None = None,
        max_bytes: int = DEFAULT_OBSERVABILITY_MAX_BYTES,
        backup_count: int = DEFAULT_OBSERVABILITY_BACKUP_COUNT,
    ) -> None:
        self._writer = JsonlEventWriter(
            path=path or get_stream_log_path(OBSERVABILITY_STREAM),
            max_bytes=max_bytes,
            backup_count=backup_count,
            error_prefix="[observability]",
        )

    def write(self, record: ObservabilityRecord) -> None:
        """Append one validated observability record."""
        self._writer.write_or_raise(record.to_record())


def get_writer() -> ObservabilityJsonlWriter:
    """Return the module-level singleton observability JSONL writer."""
    global _writer  # noqa: PLW0603
    if _writer is None:
        _writer = ObservabilityJsonlWriter()
    return _writer


__all__ = [
    "DEFAULT_OBSERVABILITY_BACKUP_COUNT",
    "DEFAULT_OBSERVABILITY_MAX_BYTES",
    "OBSERVABILITY_STREAM",
    "ObservabilityJsonlWriter",
    "get_writer",
]
