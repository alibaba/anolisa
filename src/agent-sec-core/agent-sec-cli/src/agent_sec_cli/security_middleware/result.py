"""ActionResult — unified return type for all backend executions."""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class ActionResult:
    """Structured result returned by every backend ``execute()`` call.

    Attributes:
        success:    Whether the backend operation completed without error.
        data:       Backend-specific structured data (e.g. scan findings).
        stdout:     Text output suitable for CLI passthrough / display.
        exit_code:  Numeric exit code (0 = success, non-zero = failure).
        error:      Human-readable error description (empty on success).
    """

    success: bool
    data: dict = field(default_factory=dict)
    stdout: str = ""
    exit_code: int = 0
    error: str = ""
