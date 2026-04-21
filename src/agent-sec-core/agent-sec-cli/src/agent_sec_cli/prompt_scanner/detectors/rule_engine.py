"""L1 Rule Engine detector – pattern-based scanning.

Matching pipeline:
    1. Compile all enabled rules' regex patterns at init time (cache).
    2. For each input, iterate rules and search with compiled patterns.
    3. Collect matched details; compute score from severity.
    4. Also scan decoded variants (Base64/ROT13…) if the preprocessor
       provides them.

The engine keeps patterns simple and specific to minimise false positives.
Ambiguous signals are left to the L2 ML classifier.
"""

import re
import time
from typing import Any

from agent_sec_cli.prompt_scanner.detectors.base import DetectionLayer
from agent_sec_cli.prompt_scanner.result import (
    LayerResult,
    Severity,
    ThreatDetail,
)
from agent_sec_cli.prompt_scanner.rules.loader import (
    load_builtin_injection_rules,
    load_builtin_jailbreak_rules,
)

# Severity → L1 risk score mapping
SEVERITY_SCORES: dict[Severity, float] = {
    Severity.CRITICAL: 0.95,
    Severity.HIGH: 0.80,
    Severity.MEDIUM: 0.60,
    Severity.LOW: 0.40,
}


class RuleEngine(DetectionLayer):
    """L1 detection layer: fast rule-based scanning.

    Uses compiled regex patterns for each rule.  All patterns are
    compiled at init time and reused across scans.  The engine iterates
    enabled rules, matches patterns against the input, and returns a
    :class:`LayerResult` with the highest severity score found and
    details of each matched rule.
    """

    @property
    def name(self) -> str:
        return "rule_engine"

    def __init__(self) -> None:
        self._rules: list[dict[str, Any]] = []  # compiled rule dicts
        self._load_rules()

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def detect(self, text: str, metadata: dict[str, Any] | None = None) -> LayerResult:
        """Scan *text* against all enabled rules.

        If *metadata* contains a ``"decoded_variants"`` list (produced by
        the preprocessor), each variant is also scanned.
        """
        t0 = time.perf_counter()

        # Gather all texts to scan (original + decoded variants)
        texts_to_scan = [text]
        if metadata and metadata.get("decoded_variants"):
            for variant in metadata["decoded_variants"]:
                if variant and variant != text:
                    texts_to_scan.append(variant)

        details: list[ThreatDetail] = []
        max_score = 0.0
        matched_rule_ids: set[str] = set()

        for compiled_rule in self._rules:
            if compiled_rule["id"] in matched_rule_ids:
                continue  # one hit per rule is enough

            matched_text = self._match_rule(texts_to_scan, compiled_rule)
            if matched_text is not None:
                score = SEVERITY_SCORES[compiled_rule["severity"]]
                max_score = max(max_score, score)
                matched_rule_ids.add(compiled_rule["id"])
                details.append(
                    ThreatDetail(
                        rule_id=compiled_rule["id"],
                        description=compiled_rule["description"],
                        matched_text=matched_text,
                        category=compiled_rule["category"],
                    )
                )

        elapsed = (time.perf_counter() - t0) * 1000

        return LayerResult(
            layer_name=self.name,
            detected=len(details) > 0,
            score=max_score,
            details=details,
            latency_ms=elapsed,
        )

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _load_rules(self) -> None:
        """Load built-in injection + jailbreak rules and compile patterns."""
        all_rules = load_builtin_injection_rules() + load_builtin_jailbreak_rules()
        for rule in all_rules:
            if not rule.enabled or not rule.patterns:
                continue
            compiled_patterns = [
                re.compile(p, re.IGNORECASE | re.DOTALL) for p in rule.patterns
            ]
            self._rules.append(
                {
                    "id": rule.id,
                    "name": rule.name,
                    "category": rule.category,
                    "subcategory": rule.subcategory,
                    "severity": rule.severity,
                    "description": rule.description or rule.name,
                    "patterns": compiled_patterns,
                }
            )

    @staticmethod
    def _match_rule(texts: list[str], compiled_rule: dict[str, Any]) -> str | None:
        """Try to match *compiled_rule* against any of *texts*.

        Returns the first matched text snippet, or ``None``.
        """
        for pattern in compiled_rule["patterns"]:
            for text in texts:
                m = pattern.search(text)
                if m:
                    # Clamp matched text length for safety
                    matched = m.group(0)
                    if len(matched) > 200:
                        matched = matched[:200] + "…"
                    return matched
        return None
