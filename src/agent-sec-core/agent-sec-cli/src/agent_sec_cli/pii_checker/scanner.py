"""Detector orchestration for the PII checker."""

import time
from collections import Counter
from collections.abc import Sequence

from agent_sec_cli.pii_checker.detectors.base import PiiCandidate, PiiDetector
from agent_sec_cli.pii_checker.detectors.regex import RegexPiiDetector
from agent_sec_cli.pii_checker.models import (
    PiiFinding,
    PiiScanResult,
    PiiSeverity,
    Verdict,
)
from agent_sec_cli.pii_checker.redactor import redact_text, redact_value

DEFAULT_MAX_BYTES = 1_048_576
LOW_CONFIDENCE_THRESHOLD = 0.5
ALLOWED_SOURCES = {"user_input", "tool_output", "manual", "unknown"}
_MULTI_TYPE_OVERLAPS = {frozenset({"bearer_token", "jwt"})}


def _limit_text(text: str, max_bytes: int) -> tuple[str, bool, int]:
    """Limit text by encoded byte length."""
    encoded = text.encode("utf-8")
    if len(encoded) <= max_bytes:
        return text, False, len(encoded)
    trimmed = encoded[:max_bytes].decode("utf-8", errors="ignore")
    return trimmed, True, max_bytes


def _aggregate_verdict(findings: list[PiiFinding]) -> str:
    """Aggregate findings into pass/warn/deny."""
    if any(finding.severity == PiiSeverity.DENY.value for finding in findings):
        return Verdict.DENY.value
    if findings:
        return Verdict.WARN.value
    return Verdict.PASS.value


def _overlaps(left: tuple[int, int], right: tuple[int, int]) -> bool:
    """Return whether two spans overlap."""
    return left[0] < right[1] and right[0] < left[1]


def _should_drop_overlapping(candidate: PiiCandidate, existing: PiiCandidate) -> bool:
    """Return whether an overlapping candidate is redundant."""
    if not _overlaps(candidate.span, existing.span):
        return False
    pair = frozenset({candidate.pii_type, existing.pii_type})
    if candidate.span == existing.span and pair in _MULTI_TYPE_OVERLAPS:
        return False
    return True


class PiiScanner:
    """PII scanner that orchestrates one or more detector implementations."""

    def __init__(self, detectors: Sequence[PiiDetector] | None = None) -> None:
        """Create a scanner with built-in regex detection unless overridden."""
        self._detectors = (
            list(detectors) if detectors is not None else [RegexPiiDetector()]
        )

    def scan(
        self,
        text: str,
        *,
        source: str = "unknown",
        include_low_confidence: bool = False,
        raw_evidence: bool = False,
        redact_output: bool = False,
        max_bytes: int = DEFAULT_MAX_BYTES,
    ) -> PiiScanResult:
        """Scan text and return a fixed-schema result."""
        started = time.perf_counter()
        normalized_source = source if source in ALLOWED_SOURCES else "unknown"
        limited_text, truncated, bytes_scanned = _limit_text(text, max_bytes)

        candidates = self._detect(limited_text)
        findings = self._build_findings(
            candidates,
            include_low_confidence=include_low_confidence,
            raw_evidence=raw_evidence,
        )
        verdict = _aggregate_verdict(findings)
        summary = self._build_summary(
            findings,
            source=normalized_source,
            bytes_scanned=bytes_scanned,
            truncated=truncated,
        )
        elapsed_ms = int((time.perf_counter() - started) * 1000)

        return PiiScanResult(
            ok=True,
            verdict=verdict,
            summary=summary,
            findings=findings,
            elapsed_ms=elapsed_ms,
            include_raw_evidence=raw_evidence,
            redacted_text=(
                redact_text(limited_text, findings) if redact_output else None
            ),
        )

    def _detect(self, text: str) -> list[PiiCandidate]:
        """Run configured detectors and return deduplicated raw candidates."""
        candidates: list[PiiCandidate] = []
        for detector in self._detectors:
            detector_name = getattr(detector, "name", "unknown")
            detector_engine = getattr(detector, "engine", detector_name)
            for candidate in detector.detect(text):
                if candidate.detector != "unknown" and candidate.engine != "unknown":
                    candidates.append(candidate)
                    continue
                candidates.append(
                    candidate.model_copy(
                        update={
                            "detector": (
                                detector_name
                                if candidate.detector == "unknown"
                                else candidate.detector
                            ),
                            "engine": (
                                detector_engine
                                if candidate.engine == "unknown"
                                else candidate.engine
                            ),
                        }
                    )
                )
        return self._dedupe(candidates)

    def _dedupe(self, candidates: list[PiiCandidate]) -> list[PiiCandidate]:
        """Drop redundant overlaps while preserving meaningful type enrichment."""
        ordered = sorted(
            candidates,
            key=lambda item: (
                item.severity != PiiSeverity.DENY.value,
                -item.confidence,
                item.span[0],
                -(item.span[1] - item.span[0]),
            ),
        )
        kept: list[PiiCandidate] = []
        for candidate in ordered:
            if any(_should_drop_overlapping(candidate, existing) for existing in kept):
                continue
            kept.append(candidate)
        return sorted(kept, key=lambda item: item.span[0])

    def _build_findings(
        self,
        candidates: list[PiiCandidate],
        *,
        include_low_confidence: bool,
        raw_evidence: bool,
    ) -> list[PiiFinding]:
        """Convert candidates to public findings."""
        findings: list[PiiFinding] = []
        for candidate in candidates:
            if (
                not include_low_confidence
                and candidate.confidence < LOW_CONFIDENCE_THRESHOLD
            ):
                continue
            metadata = dict(candidate.metadata)
            metadata.setdefault("detector", candidate.detector)
            metadata.setdefault("engine", candidate.engine)
            findings.append(
                PiiFinding(
                    type=candidate.pii_type,
                    category=candidate.category,
                    severity=candidate.severity,
                    confidence=candidate.confidence,
                    evidence_redacted=redact_value(candidate.value, candidate.pii_type),
                    span=candidate.span,
                    metadata=metadata,
                    raw_evidence=candidate.value if raw_evidence else None,
                )
            )
        return findings

    def _build_summary(
        self,
        findings: list[PiiFinding],
        *,
        source: str,
        bytes_scanned: int,
        truncated: bool,
    ) -> dict[str, object]:
        """Build aggregate summary data."""
        by_type = Counter(finding.type for finding in findings)
        by_category = Counter(finding.category for finding in findings)
        by_severity = Counter(finding.severity for finding in findings)
        return {
            "total": len(findings),
            "by_type": dict(sorted(by_type.items())),
            "by_category": dict(sorted(by_category.items())),
            "by_severity": dict(sorted(by_severity.items())),
            "source": source,
            "bytes_scanned": bytes_scanned,
            "truncated": truncated,
        }


def scan_text(
    text: str,
    *,
    detectors: Sequence[PiiDetector] | None = None,
    source: str = "unknown",
    include_low_confidence: bool = False,
    raw_evidence: bool = False,
    redact_output: bool = False,
    max_bytes: int = DEFAULT_MAX_BYTES,
) -> PiiScanResult:
    """Convenience function for one-off scans."""
    return PiiScanner(detectors=detectors).scan(
        text,
        source=source,
        include_low_confidence=include_low_confidence,
        raw_evidence=raw_evidence,
        redact_output=redact_output,
        max_bytes=max_bytes,
    )
