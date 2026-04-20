"""Unit tests for prompt_scanner.rules.loader — YAML rule loading."""

import tempfile
import unittest
from pathlib import Path

from agent_sec_cli.prompt_scanner.exceptions import ConfigError
from agent_sec_cli.prompt_scanner.result import Severity
from agent_sec_cli.prompt_scanner.rules.loader import (
    Rule,
    load_builtin_injection_rules,
    load_builtin_jailbreak_rules,
    load_rules_from_yaml,
)


class TestRuleModel(unittest.TestCase):
    """Tests for the Rule pydantic model."""

    def test_rule_creation(self):
        rule = Rule(
            id="TEST-001",
            name="Test Rule",
            category="direct_injection",
            subcategory="test",
            severity=Severity.HIGH,
            patterns=[r"(?i)test\s+pattern"],
            keywords=["test"],
            description="A test rule",
        )
        self.assertEqual(rule.id, "TEST-001")
        self.assertEqual(rule.severity, Severity.HIGH)
        self.assertTrue(rule.enabled)

    def test_rule_defaults(self):
        rule = Rule(
            id="TEST-002",
            name="Minimal",
            category="jailbreak",
            subcategory="test",
            severity=Severity.LOW,
        )
        self.assertEqual(rule.patterns, [])
        self.assertEqual(rule.keywords, [])
        self.assertEqual(rule.description, "")
        self.assertTrue(rule.enabled)


class TestLoadRulesFromYaml(unittest.TestCase):
    """Tests for the YAML rule loader."""

    def test_load_valid_yaml(self):
        yaml_content = """\
rules:
  - id: TEST-001
    name: "Test Rule"
    category: direct_injection
    subcategory: test
    severity: critical
    patterns:
      - '(?i)test\\s+pattern'
    keywords:
      - "test"
    description: "A test rule"
"""
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write(yaml_content)
            f.flush()
            rules = load_rules_from_yaml(Path(f.name))

        self.assertEqual(len(rules), 1)
        self.assertEqual(rules[0].id, "TEST-001")
        self.assertEqual(rules[0].severity, Severity.CRITICAL)
        self.assertEqual(rules[0].patterns, ["(?i)test\\s+pattern"])

    def test_load_file_not_found(self):
        with self.assertRaises(FileNotFoundError):
            load_rules_from_yaml(Path("/nonexistent/rules.yaml"))

    def test_load_invalid_yaml_syntax(self):
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write("rules: [invalid yaml {{{")
            f.flush()
            with self.assertRaises(ConfigError):
                load_rules_from_yaml(Path(f.name))

    def test_load_missing_rules_key(self):
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write("other_key: []\n")
            f.flush()
            with self.assertRaises(ConfigError):
                load_rules_from_yaml(Path(f.name))

    def test_load_rules_not_list(self):
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write("rules: not_a_list\n")
            f.flush()
            with self.assertRaises(ConfigError):
                load_rules_from_yaml(Path(f.name))

    def test_load_invalid_rule_field(self):
        yaml_content = """\
rules:
  - id: TEST-001
    name: "Bad Rule"
    category: direct_injection
    subcategory: test
    severity: not_a_real_severity
    patterns: []
"""
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write(yaml_content)
            f.flush()
            with self.assertRaises(ConfigError):
                load_rules_from_yaml(Path(f.name))

    def test_load_rule_not_mapping(self):
        yaml_content = """\
rules:
  - "just a string"
"""
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write(yaml_content)
            f.flush()
            with self.assertRaises(ConfigError):
                load_rules_from_yaml(Path(f.name))

    def test_load_multiple_rules(self):
        yaml_content = """\
rules:
  - id: TEST-001
    name: "Rule One"
    category: direct_injection
    subcategory: test
    severity: high
    patterns: ['(?i)one']
  - id: TEST-002
    name: "Rule Two"
    category: jailbreak
    subcategory: test
    severity: critical
    patterns: ['(?i)two']
"""
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".yaml", delete=False, encoding="utf-8"
        ) as f:
            f.write(yaml_content)
            f.flush()
            rules = load_rules_from_yaml(Path(f.name))

        self.assertEqual(len(rules), 2)
        self.assertEqual(rules[0].id, "TEST-001")
        self.assertEqual(rules[1].id, "TEST-002")


class TestLoadBuiltinRules(unittest.TestCase):
    """Tests for loading built-in YAML rule files."""

    def test_load_injection_rules(self):
        rules = load_builtin_injection_rules()
        self.assertGreater(len(rules), 0)
        for rule in rules:
            self.assertTrue(rule.id.startswith("INJ-"))
            self.assertIn(rule.category, ("direct_injection", "indirect_injection"))
            self.assertIsInstance(rule.severity, Severity)
            self.assertIsInstance(rule.patterns, list)
            self.assertGreater(len(rule.patterns), 0)

    def test_load_jailbreak_rules(self):
        rules = load_builtin_jailbreak_rules()
        self.assertGreater(len(rules), 0)
        for rule in rules:
            self.assertTrue(rule.id.startswith("JB-"))
            self.assertEqual(rule.category, "jailbreak")

    def test_builtin_rules_have_valid_regex(self):
        """Verify all built-in patterns compile without error."""
        import re

        all_rules = load_builtin_injection_rules() + load_builtin_jailbreak_rules()
        for rule in all_rules:
            for pattern in rule.patterns:
                try:
                    re.compile(pattern)
                except re.error as exc:
                    self.fail(f"Invalid regex in rule {rule.id}: {pattern!r} — {exc}")

    def test_injection_rule_ids_unique(self):
        rules = load_builtin_injection_rules()
        ids = [r.id for r in rules]
        self.assertEqual(len(ids), len(set(ids)), "Duplicate injection rule IDs")

    def test_jailbreak_rule_ids_unique(self):
        rules = load_builtin_jailbreak_rules()
        ids = [r.id for r in rules]
        self.assertEqual(len(ids), len(set(ids)), "Duplicate jailbreak rule IDs")


if __name__ == "__main__":
    unittest.main()
