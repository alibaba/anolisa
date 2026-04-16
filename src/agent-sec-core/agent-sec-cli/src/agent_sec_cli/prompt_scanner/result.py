"""Result data structures for prompt scanner."""

from enum import Enum
from typing import Any

from pydantic import BaseModel, Field


class ThreatType(str, Enum):
    """Type of detected threat.

    - DIRECT_INJECTION:   User input directly contains injection payload.
    - INDIRECT_INJECTION: Injection payload delivered via indirect channels
                          (RAG retrieval, tool output, memory/context injection)
                          — also known as IPI (Indirect Prompt Injection).
    - JAILBREAK:          Attempt to bypass safety restrictions or role-play.
    - BENIGN:             No threat detected.
    """

    DIRECT_INJECTION = "direct_injection"
    INDIRECT_INJECTION = "indirect_injection"
    JAILBREAK = "jailbreak"
    BENIGN = "benign"


class Severity(str, Enum):
    """Severity level for a detection rule or finding."""

    LOW = "low"
    MEDIUM = "medium"
    HIGH = "high"
    CRITICAL = "critical"


class Verdict(str, Enum):
    """Final verdict of a scan.

    - PASS: No notable injection characteristics found.
    - WARN: Suspicious prompt injection detected.
    - DENY: High-risk injection detected.
    - ERROR: Scanner execution failed.
    """

    PASS = "pass"
    WARN = "warn"
    DENY = "deny"
    ERROR = "error"


class ThreatDetail(BaseModel):
    """Detail of a single threat finding."""

    rule_id: str  # e.g. "INJ-001"
    description: str  # Human-readable explanation
    matched_text: str  # The text snippet that matched
    category: str  # Attack category


class LayerResult(BaseModel):
    """Result from a single detection layer."""

    layer_name: str  # e.g. "rule_engine", "ml_classifier"
    detected: bool  # Whether this layer detected a threat
    score: float  # Risk score from this layer (0.0 - 1.0)
    details: list[ThreatDetail] = Field(default_factory=list)
    latency_ms: float = 0.0


class ScanResult(BaseModel):
    """Aggregated result of a prompt scan across all layers."""

    is_threat: bool  # Whether a threat was detected
    threat_type: ThreatType  # INJECTION / JAILBREAK / BENIGN
    risk_score: float  # Overall risk score (0.0 - 1.0)
    confidence: float  # Confidence of the result (0.0 - 1.0)
    layer_results: list[LayerResult] = Field(default_factory=list)
    latency_ms: float = 0.0
    metadata: dict[str, Any] = Field(default_factory=dict)
    verdict: Verdict = Verdict.PASS

    def to_dict(self) -> dict[str, Any]:
        """Serialize to the CLI JSON output format.

        Output schema follows the design spec (schema_version 1.0).
        """
        findings = []
        for lr in self.layer_results:
            for detail in lr.details:
                findings.append(
                    {
                        "rule_id": detail.rule_id,
                        "severity": _score_to_severity(lr.score).value,
                        "title": detail.description,
                        "message": detail.description,
                        "evidence": detail.matched_text,
                    }
                )

        return {
            "schema_version": "1.0",
            "ok": not self.is_threat,
            "verdict": self.verdict.value,
            "risk_level": _score_to_severity(self.risk_score).value,
            "summary": self._build_summary(),
            "findings": findings,
            "engine_version": "0.1.0",
            "elapsed_ms": round(self.latency_ms, 2),
        }

    def _build_summary(self) -> str:
        if not self.is_threat:
            return "No threats detected"
        return f"Potential prompt {self.threat_type.value} detected"


def _score_to_severity(score: float) -> Severity:
    """Map a 0-1 risk score to a severity level."""
    if score >= 0.9:
        return Severity.CRITICAL
    if score >= 0.7:
        return Severity.HIGH
    if score >= 0.4:
        return Severity.MEDIUM
    return Severity.LOW
