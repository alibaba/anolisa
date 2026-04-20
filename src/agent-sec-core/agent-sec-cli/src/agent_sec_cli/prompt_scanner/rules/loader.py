"""YAML rule loader – parse and validate rule definitions from YAML files."""

from pathlib import Path

import yaml
from agent_sec_cli.prompt_scanner.exceptions import ConfigError
from agent_sec_cli.prompt_scanner.result import Severity
from pydantic import BaseModel, Field


class Rule(BaseModel):
    """A single detection rule used by the L1 rule engine."""

    id: str  # Unique identifier, e.g. "INJ-001"
    name: str  # Human-readable rule name
    category: str  # "direct_injection" / "indirect_injection" / "jailbreak"
    subcategory: str  # e.g. "instruction_override"
    severity: Severity
    patterns: list[str] = Field(default_factory=list)  # Regex patterns
    keywords: list[str] = Field(default_factory=list)  # Fast pre-filter tokens
    description: str = ""
    enabled: bool = True


_RULES_DIR = Path(__file__).parent


def load_rules_from_yaml(path: str | Path) -> list[Rule]:
    """Load and validate rules from a YAML file.

    Args:
        path: Path to the YAML file containing a ``rules:`` list.

    Returns:
        A list of validated Rule objects.

    Raises:
        FileNotFoundError: If *path* does not exist.
        ConfigError: If the file is malformed or rules fail validation.
    """
    path = Path(path)
    if not path.exists():
        raise FileNotFoundError(f"Rule file not found: {path}")

    try:
        with path.open(encoding="utf-8") as f:
            data = yaml.safe_load(f)
    except yaml.YAMLError as exc:
        raise ConfigError(f"Invalid YAML in {path}: {exc}") from exc

    if not isinstance(data, dict) or "rules" not in data:
        raise ConfigError(f"Rule file must contain a 'rules' key: {path}")

    raw_rules = data["rules"]
    if not isinstance(raw_rules, list):
        raise ConfigError(f"'rules' must be a list in {path}")

    rules: list[Rule] = []
    for idx, raw in enumerate(raw_rules):
        if not isinstance(raw, dict):
            raise ConfigError(f"Rule #{idx} is not a mapping in {path}")
        try:
            rules.append(Rule(**raw))
        except Exception as exc:
            raise ConfigError(
                f"Invalid rule #{idx} (id={raw.get('id', '?')}) in {path}: {exc}"
            ) from exc

    return rules


def load_builtin_injection_rules() -> list[Rule]:
    """Load the built-in injection rules from ``injection.yaml``."""
    return load_rules_from_yaml(_RULES_DIR / "injection.yaml")


def load_builtin_jailbreak_rules() -> list[Rule]:
    """Load the built-in jailbreak rules from ``jailbreak.yaml``."""
    return load_rules_from_yaml(_RULES_DIR / "jailbreak.yaml")
