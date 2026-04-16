"""Scoring strategy – aggregate layer results into a unified risk score."""

from agent_sec_cli.prompt_scanner.result import LayerResult, Verdict

# Layer weight map.
# ML classifier (L2) has highest trust -> 1.0
# Rule engine (L1) more prone to FP    -> 0.7
# Note: L3 (semantic) is planned but not yet implemented.
WEIGHTS = {
    "rule_engine": 0.7,
    "ml_classifier": 1.0,
}


def compute_score(layer_results: list[LayerResult]) -> float:
    """Compute the final risk score using a weighted-max strategy.

    Each layer's score is multiplied by its weight and the maximum
    weighted score is returned.  This ensures that a single high-
    confidence detection from any layer triggers an alert.

    Returns 0.0 if *layer_results* is empty.
    """
    if not layer_results:
        return 0.0
    weighted = [lr.score * WEIGHTS.get(lr.layer_name, 1.0) for lr in layer_results]
    return max(weighted)


def determine_verdict(risk_score: float, threshold: float = 0.5) -> Verdict:
    """Map a risk score to a Verdict.

    - score < threshold        -> PASS
    - threshold <= score < 0.8 -> WARN
    - score >= 0.8             -> DENY
    """
    if risk_score < threshold:
        return Verdict.PASS
    if risk_score < 0.8:
        return Verdict.WARN
    return Verdict.DENY
