"""Built-in rules for prompt scanner.

Rules are defined in YAML files in this directory:
  - injection.yaml  — prompt injection patterns (INJ-*)
  - jailbreak.yaml  — jailbreak patterns (JB-*)

The rule_engine detector loads these files directly at runtime.
"""

from agent_sec_cli.prompt_scanner.rules.loader import (
    Rule,
    load_builtin_injection_rules,
    load_builtin_jailbreak_rules,
    load_rules_from_yaml,
)

__all__ = [
    "Rule",
    "load_builtin_injection_rules",
    "load_builtin_jailbreak_rules",
    "load_rules_from_yaml",
]
