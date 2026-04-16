"""Security audit logger for prompt scanner events."""

import logging

from agent_sec_cli.prompt_scanner.result import ScanResult

logger = logging.getLogger("prompt_scanner.audit")


class AuditLogger:
    """Records scan events for security auditing and compliance.

    All threat detections and scanner errors are logged with enough
    context for post-incident analysis.

    This is a stub – structured logging, file rotation, and SIEM
    integration will be added in Commit 5.
    """

    def __init__(self, log_path: str | None = None) -> None:
        self._log_path = log_path

    def log_scan(self, result: ScanResult, prompt_text: str = "") -> None:
        """Log a completed scan event.  (stub)"""
        level = logging.WARNING if result.is_threat else logging.INFO
        logger.log(
            level,
            "scan verdict=%s risk=%.2f threat_type=%s latency=%.1fms",
            result.verdict.value,
            result.risk_score,
            result.threat_type.value,
            result.latency_ms,
        )

    def log_threat(self, result: ScanResult, prompt_text: str = "") -> None:
        """Log a threat detection event with additional detail.  (stub)"""
        logger.warning(
            "THREAT DETECTED verdict=%s risk=%.2f type=%s findings=%d",
            result.verdict.value,
            result.risk_score,
            result.threat_type.value,
            sum(len(lr.details) for lr in result.layer_results),
        )
