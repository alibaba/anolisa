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
    layer_results: list[LayerResult] = Field(default_factory=list)
    latency_ms: float = 0.0
    metadata: dict[str, Any] = Field(default_factory=dict)
    verdict: Verdict = Verdict.PASS

    def to_dict(self) -> dict[str, Any]:
        """Serialize to the CLI JSON output format.

        Output schema follows the design spec (schema_version 1.0).

        Fields:
            schema_version: Always "1.0".
            ok:             True when no threat detected.
            verdict:        PASS / WARN / DENY / ERROR.
            risk_level:     critical / high / medium / low (derived from verdict).
            threat_type:    direct_injection / indirect_injection / jailbreak / benign.
            confidence:     Best available confidence (ML softmax prob when available,
                            otherwise highest rule-engine score).  None when PASS/ERROR.
            summary:        Human-readable one-liner.
            findings:       List of individual rule/model hits.
            layer_results:  Per-layer breakdown (name, detected, score, latency_ms).
            engine_version: Semantic version string.
            elapsed_ms:     Total scan time in milliseconds.
        """
        findings = []
        for lr in self.layer_results:
            for detail in lr.details:
                findings.append(
                    {
                        "rule_id": detail.rule_id,
                        "title": detail.description,
                        "message": detail.description,
                        "evidence": detail.matched_text,
                        "category": detail.category,
                    }
                )

        layer_summary = [
            {
                "layer": lr.layer_name,
                "detected": lr.detected,
                "score": round(lr.score, 4),
                "latency_ms": round(lr.latency_ms, 2),
            }
            for lr in self.layer_results
        ]

        return {
            "schema_version": "1.0",
            "ok": not self.is_threat,
            "verdict": self.verdict.value,
            "risk_level": _verdict_to_risk_level(self.verdict),
            "threat_type": self.threat_type.value,
            **(
                {"confidence": round(_best_confidence(self.layer_results), 3)}
                if self.is_threat
                else {}
            ),
            "summary": self._build_summary(),
            "findings": findings,
            "layer_results": layer_summary,
            "engine_version": "0.1.0",
            "elapsed_ms": round(self.latency_ms, 2),
        }

    def _build_summary(self) -> str:  # noqa: PLR0912
        """Build a human-readable one-liner that explains the scan outcome.

        Format (threat detected)::

            [<layers>] <ThreatType> detected (confidence: <N>%) — "<evidence>"

        Format (clean)::

            No threats detected (benign probability: <N>%)

        The confidence figure comes from the ML layer when available (model-
        backed softmax probability), otherwise from the rule-engine score.
        Evidence is the first matched_text snippet, truncated to 60 chars.
        """
        if not self.is_threat:
            # Try to surface the ML benign confidence so the user has a signal.
            for lr in self.layer_results:
                if lr.layer_name == "ml_classifier" and not lr.detected:
                    pct = round(lr.score * 100, 1) if lr.score else None
                    if pct is not None:
                        # lr.score for a benign result is the *jailbreak* prob,
                        # so benign probability = 1 - score.
                        benign_pct = round((1.0 - lr.score) * 100, 1)
                        return (
                            f"No threats detected (ML benign confidence: {benign_pct}%)"
                        )
            return "No threats detected"

        # --- Threat path ---
        # 1. Which layers fired?
        fired_layers = [
            _LAYER_SHORT.get(lr.layer_name, lr.layer_name)
            for lr in self.layer_results
            if lr.detected
        ]
        layer_tag = "+".join(fired_layers) if fired_layers else "unknown"

        # 2. Confidence: reuse shared helper.
        raw_conf = _best_confidence(self.layer_results)
        confidence_pct = round(raw_conf * 100, 1) if raw_conf else None

        # 3. First evidence snippet (truncated).
        evidence: str | None = None
        for lr in self.layer_results:
            if lr.detected and lr.details:
                raw = lr.details[0].matched_text.strip()
                evidence = raw[:60] + ("…" if len(raw) > 60 else "")
                break

        threat_label = self.threat_type.value.replace("_", " ").title()
        base = f"[{layer_tag}] {threat_label} detected (confidence: {confidence_pct}%)"
        if evidence:
            base = f'{base} — "{evidence}"'
        return base


# Short display names for layer tags in the summary.
_LAYER_SHORT: dict[str, str] = {
    "rule_engine": "Rule",
    "ml_classifier": "ML",
    "semantic": "Semantic",
}


def _verdict_to_risk_level(verdict: Verdict) -> str:
    """Map a Verdict to a risk_level string for the JSON output."""
    return {
        Verdict.PASS: "low",
        Verdict.WARN: "medium",
        Verdict.DENY: "high",
        Verdict.ERROR: "unknown",
    }.get(verdict, "unknown")


def _best_confidence(layer_results: list[LayerResult]) -> float:
    """Return the best available confidence value from detected layers.

    Prefers the ML classifier score (model-backed softmax probability) over
    rule-engine scores.  Falls back to the highest score among all detected
    layers when no ML result is present.
    """
    for lr in layer_results:
        if lr.layer_name == "ml_classifier" and lr.detected:
            return lr.score
    scores = [lr.score for lr in layer_results if lr.detected]
    return max(scores) if scores else 0.0
