"""Verdict derivation – map layer detection results to a final Verdict.

Verdict semantics
-----------------
PASS   No layer detected a threat.  Message continues unchanged.
WARN   L1 (rule engine) fired but L2 (ML) was present and did NOT
       confirm.  Regex heuristic may be a false-positive; log and
       alert, do not treat as definitive.
DENY   L2 (ML classifier) confirmed a threat, OR L1 fired in FAST
       mode where no L2 layer ran (L1 is the sole authority).
       Log as high-severity event.
ERROR  Scanner itself failed.  Log the exception; pass the original
       message through so the pipeline is not broken.
"""

from agent_sec_cli.prompt_scanner.result import LayerResult, Verdict

# Layers whose detection is treated as a confirmed threat → DENY.
# L1 (rule_engine) alone → WARN because regex has a higher false-positive
# rate and its signal should be confirmed by L2 when L2 is present.
_CONFIRM_LAYERS = frozenset({"ml_classifier"})


def determine_verdict(layer_results: list[LayerResult]) -> Verdict:
    """Derive a Verdict from detection results across all layers.

    Decision rules (evaluated in order):

    1. Any confirm-layer (L2 ML) detected → **DENY**
    2. L1 detected AND no confirm-layer was present (FAST mode) → **DENY**
       L1 is the sole authority when L2 has not run.
    3. L1 detected AND confirm-layer was present but did not fire → **WARN**
       ML did not confirm the regex signal; treat as possible false-positive.
    4. No layer detected → **PASS**

    Args:
        layer_results: Ordered list of per-layer results from the scanner.

    Returns:
        The applicable ``Verdict``.
    """
    confirmed = any(
        lr.detected and lr.layer_name in _CONFIRM_LAYERS for lr in layer_results
    )
    if confirmed:
        return Verdict.DENY

    any_detected = any(lr.detected for lr in layer_results)
    if any_detected:
        # L1 fired.  Check whether a confirm-layer (L2) actually ran.
        confirm_layer_ran = any(
            lr.layer_name in _CONFIRM_LAYERS for lr in layer_results
        )
        if confirm_layer_ran:
            # L2 ran but did not confirm → WARN (possible L1 false-positive)
            return Verdict.WARN
        else:
            # FAST mode: no L2, L1 is sole authority → DENY
            return Verdict.DENY

    return Verdict.PASS
