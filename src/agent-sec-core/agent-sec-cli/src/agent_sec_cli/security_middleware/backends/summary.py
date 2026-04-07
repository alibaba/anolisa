"""Summary backend — future stub for security event aggregation."""

from __future__ import annotations

from agent_sec_cli.security_middleware.result import ActionResult


class SummaryBackend:
    """Placeholder for security event summary and reporting.

    This backend will eventually aggregate events from the JSONL log
    and produce time-windowed, categorised reports.
    """

    def execute(self, ctx, **kwargs) -> ActionResult:
        """Not yet implemented — always returns failure."""
        return ActionResult(
            success=False,
            error="Security event summary not yet implemented",
        )
