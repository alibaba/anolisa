"""Unit tests for scanner registry and result parsers.

These tests protect:
1. Parser normalization — the data ingestion boundary for all external scanner output.
   Malformed/unexpected input must be handled gracefully, not crash the pipeline.
2. Registry lookup chain — scanner → parser name → parser info resolution.
   Fallback to findings-array for unknown scanners is a backward compat contract.
3. Status derivation from findings — deny > warn > pass is the per-scanner logic.
"""

import unittest

from agent_sec_cli.skill_ledger.core.certifier import _determine_scan_status
from agent_sec_cli.skill_ledger.models.finding import NormalizedFinding
from agent_sec_cli.skill_ledger.scanner.parsers import parse_findings
from agent_sec_cli.skill_ledger.scanner.registry import (
    ParserInfo,
    ScannerInfo,
    ScannerRegistry,
)


class TestFindingsArrayParser(unittest.TestCase):
    """The findings-array parser is the identity parser — input is already
    in standard format.  But real-world data is messy.  These tests verify
    that the parser handles edge cases without crashing the certify pipeline.
    """

    def test_valid_findings_parsed(self):
        raw = [
            {"rule": "dangerous-exec", "level": "deny", "message": "exec found"},
            {"rule": "obfuscated", "level": "warn", "message": "hex encoding"},
        ]
        result = parse_findings(raw, None)
        self.assertEqual(len(result), 2)
        self.assertEqual(result[0].rule, "dangerous-exec")
        self.assertEqual(result[0].level, "deny")
        self.assertEqual(result[1].level, "warn")

    def test_missing_rule_skipped(self):
        """Findings without 'rule' are invalid — skip, don't crash."""
        raw = [
            {"level": "warn", "message": "no rule"},
            {"rule": "valid", "level": "pass", "message": "ok"},
        ]
        result = parse_findings(raw, None)
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0].rule, "valid")

    def test_missing_level_skipped(self):
        """Findings without 'level' are invalid — skip, don't crash."""
        raw = [{"rule": "r1", "message": "no level"}]
        result = parse_findings(raw, None)
        self.assertEqual(len(result), 0)

    def test_unknown_level_normalized_to_warn(self):
        """Unknown level strings are treated as 'warn' — safe conservative default."""
        raw = [{"rule": "r1", "level": "HIGH", "message": "unknown"}]
        result = parse_findings(raw, None)
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0].level, "warn")

    def test_level_case_insensitive(self):
        """Level matching is case-insensitive — 'DENY' should work like 'deny'."""
        raw = [{"rule": "r1", "level": "DENY", "message": "caps"}]
        result = parse_findings(raw, None)
        self.assertEqual(result[0].level, "deny")

    def test_extra_fields_captured_in_metadata(self):
        """Scanner-specific fields not in the model are preserved, not dropped."""
        raw = [
            {
                "rule": "r1",
                "level": "pass",
                "message": "ok",
                "severity_score": 0.9,
                "cwe_id": "CWE-78",
            }
        ]
        result = parse_findings(raw, None)
        self.assertIn("severity_score", result[0].metadata)
        self.assertIn("cwe_id", result[0].metadata)
        self.assertEqual(result[0].metadata["severity_score"], 0.9)

    def test_non_dict_items_skipped(self):
        """Non-dict items in the findings list are skipped — handles garbage input."""
        raw = [
            "not a dict",
            42,
            {"rule": "valid", "level": "pass", "message": "ok"},
        ]
        result = parse_findings(raw, None)
        self.assertEqual(len(result), 1)
        self.assertEqual(result[0].rule, "valid")

    def test_empty_list_returns_empty(self):
        result = parse_findings([], None)
        self.assertEqual(result, [])


class TestParserDispatch(unittest.TestCase):
    """parse_findings() dispatches by parser type. Unknown types fall back safely."""

    def test_none_parser_uses_findings_array(self):
        """No parser info → fall back to findings-array (backward compat)."""
        raw = [{"rule": "r1", "level": "pass", "message": "ok"}]
        result = parse_findings(raw, None)
        self.assertEqual(len(result), 1)

    def test_findings_array_parser_dispatches(self):
        parser = ParserInfo(name="findings-array", type="findings-array")
        raw = [{"rule": "r1", "level": "deny", "message": "bad"}]
        result = parse_findings(raw, parser)
        self.assertEqual(result[0].level, "deny")

    def test_unknown_parser_type_falls_back(self):
        """Future parser types not yet implemented → fall back to findings-array."""
        parser = ParserInfo(name="sarif-future", type="sarif")
        raw = [{"rule": "r1", "level": "warn", "message": "m"}]
        result = parse_findings(raw, parser)
        self.assertEqual(len(result), 1)


class TestScannerRegistry(unittest.TestCase):
    """Registry is the configuration backbone — tests ensure lookup correctness."""

    def _make_registry(self):
        config = {
            "scanners": [
                {"name": "skill-vetter", "type": "skill", "parser": "findings-array"},
                {
                    "name": "pattern-scanner",
                    "type": "builtin",
                    "parser": "findings-array",
                },
                {"name": "disabled-one", "type": "cli", "enabled": False},
            ],
            "parsers": {
                "findings-array": {"type": "findings-array"},
            },
        }
        return ScannerRegistry.from_config(config)

    def test_get_scanner_returns_info(self):
        reg = self._make_registry()
        sv = reg.get_scanner("skill-vetter")
        self.assertIsNotNone(sv)
        self.assertEqual(sv.type, "skill")
        self.assertEqual(sv.parser, "findings-array")

    def test_get_scanner_unknown_returns_none(self):
        reg = self._make_registry()
        self.assertIsNone(reg.get_scanner("nonexistent"))

    def test_get_parser_for_scanner_chain(self):
        """scanner → parser name → parser info lookup chain must work."""
        reg = self._make_registry()
        pi = reg.get_parser_for_scanner("skill-vetter")
        self.assertIsNotNone(pi)
        self.assertEqual(pi.type, "findings-array")

    def test_get_parser_for_unknown_scanner_returns_none(self):
        reg = self._make_registry()
        self.assertIsNone(reg.get_parser_for_scanner("unknown"))

    def test_list_invocable_excludes_skill_type(self):
        """skill-type scanners require Agent — CLI must not auto-invoke them."""
        reg = self._make_registry()
        invocable = reg.list_invocable_scanners()
        names = [s.name for s in invocable]
        self.assertNotIn("skill-vetter", names)
        self.assertIn("pattern-scanner", names)

    def test_list_invocable_excludes_disabled(self):
        reg = self._make_registry()
        invocable = reg.list_invocable_scanners()
        names = [s.name for s in invocable]
        self.assertNotIn("disabled-one", names)

    def test_list_invocable_with_name_filter(self):
        reg = self._make_registry()
        invocable = reg.list_invocable_scanners(names=["pattern-scanner"])
        self.assertEqual(len(invocable), 1)
        self.assertEqual(invocable[0].name, "pattern-scanner")


class TestDetermineStatusFromFindings(unittest.TestCase):
    """Per-scanner status derived from normalized findings — deny > warn > pass."""

    def test_empty_findings_returns_pass(self):
        self.assertEqual(_determine_scan_status([]), "pass")

    def test_all_pass_returns_pass(self):
        findings = [NormalizedFinding(rule="r1", level="pass", message="ok")]
        self.assertEqual(_determine_scan_status(findings), "pass")

    def test_deny_present_returns_deny(self):
        findings = [
            NormalizedFinding(rule="r1", level="pass", message="ok"),
            NormalizedFinding(rule="r2", level="deny", message="bad"),
        ]
        self.assertEqual(_determine_scan_status(findings), "deny")

    def test_warn_without_deny_returns_warn(self):
        findings = [
            NormalizedFinding(rule="r1", level="pass", message="ok"),
            NormalizedFinding(rule="r2", level="warn", message="iffy"),
        ]
        self.assertEqual(_determine_scan_status(findings), "warn")


if __name__ == "__main__":
    unittest.main()
