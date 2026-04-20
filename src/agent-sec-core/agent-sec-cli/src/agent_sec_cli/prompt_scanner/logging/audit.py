"""Security audit logger for prompt scanner events."""

import json
import logging
import time
from pathlib import Path
from typing import Any

from agent_sec_cli.prompt_scanner.result import ScanResult

_audit_logger = logging.getLogger("prompt_scanner.audit")


class AuditLogger:
    """Records scan events for security auditing and compliance.

    Emits structured log records via the standard ``logging`` module so they
    can be captured by any handler (console, file, SIEM forwarder, etc.).
    Optionally appends JSONL records to a dedicated audit file.

    Usage::

        audit = AuditLogger()                       # logger only
        audit = AuditLogger(log_path="/var/log/agent-sec/audit.jsonl")
        audit.log_scan(result)
        audit.log_threat(result, prompt_text=raw)
    """

    def __init__(self, log_path: str | None = None) -> None:
        self._log_path: Path | None = Path(log_path) if log_path else None
        if self._log_path:
            self._log_path.parent.mkdir(parents=True, exist_ok=True)

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def log_scan(self, result: ScanResult, prompt_text: str = "") -> None:
        """Log a completed scan event at INFO (benign) or WARNING (threat) level."""
        level = logging.WARNING if result.is_threat else logging.INFO
        _audit_logger.log(
            level,
            "scan verdict=%s risk=%.2f threat_type=%s latency=%.1fms",
            result.verdict.value,
            result.risk_score,
            result.threat_type.value,
            result.latency_ms,
        )
        self._write_jsonl(
            event="scan",
            verdict=result.verdict.value,
            risk_score=round(result.risk_score, 4),
            threat_type=result.threat_type.value,
            is_threat=result.is_threat,
            latency_ms=round(result.latency_ms, 2),
            finding_count=sum(len(lr.details) for lr in result.layer_results),
            prompt_length=len(prompt_text),
        )

    def log_threat(self, result: ScanResult, prompt_text: str = "") -> None:
        """Log a threat detection event with per-finding detail at WARNING level."""
        findings = [
            {
                "rule_id": detail.rule_id,
                "category": detail.category,
                "matched": detail.matched_text[:120],
            }
            for lr in result.layer_results
            for detail in lr.details
        ]
        _audit_logger.warning(
            "THREAT DETECTED verdict=%s risk=%.2f type=%s findings=%d",
            result.verdict.value,
            result.risk_score,
            result.threat_type.value,
            len(findings),
        )
        self._write_jsonl(
            event="threat",
            verdict=result.verdict.value,
            risk_score=round(result.risk_score, 4),
            threat_type=result.threat_type.value,
            latency_ms=round(result.latency_ms, 2),
            findings=findings,
            prompt_length=len(prompt_text),
        )

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _write_jsonl(self, event: str, **fields: Any) -> None:
        """Append a single JSONL record to *log_path* (no-op when not configured)."""
        if self._log_path is None:
            return
        record: dict[str, Any] = {
            "ts": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "event": event,
            **fields,
        }
        with self._log_path.open("a", encoding="utf-8") as fh:
            fh.write(json.dumps(record, ensure_ascii=False) + "\n")
