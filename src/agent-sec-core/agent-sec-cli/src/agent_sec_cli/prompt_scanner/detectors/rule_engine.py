"""L1 Rule Engine detector – pattern & keyword matching."""

from agent_sec_cli.prompt_scanner.detectors.base import DetectionLayer
from agent_sec_cli.prompt_scanner.result import LayerResult


class RuleEngine(DetectionLayer):
    """L1 detection layer: fast rule-based scanning.

    Uses Aho-Corasick multi-pattern matching for keywords and compiled
    regex patterns.  This is a stub – real rule loading and matching
    logic will be implemented in a subsequent commit.
    """

    @property
    def name(self) -> str:
        return "rule_engine"

    def __init__(self) -> None:
        self._rules: list = []
        self._load_rules()

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def detect(self, text: str, metadata: dict | None = None) -> LayerResult:
        """Scan *text* against loaded rules.  (stub)"""
        # TODO: implement keyword + regex + custom_fn matching pipeline
        return LayerResult(layer_name=self.name, detected=False, score=0.0)

    # ------------------------------------------------------------------
    # Internal helpers (signatures only – implementation deferred)
    # ------------------------------------------------------------------

    def _load_rules(self) -> None:
        """Load built-in and custom rules into the engine."""
        # TODO: populate self._rules from injection_rules + jailbreak_rules
        pass

    def _keyword_match(self, text: str) -> list[str]:
        """Return list of matched keyword rule IDs.  (stub)"""
        # TODO: Aho-Corasick automaton
        return []

    def _regex_match(self, text: str, rule_ids: list[str]) -> list[str]:
        """Validate keyword hits with regex patterns.  (stub)"""
        return []
