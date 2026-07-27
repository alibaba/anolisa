"""Stable scanner identifiers and legacy aliases for skill-ledger."""

CODE_SCANNER_NAME = "code-scanner"
STATIC_SCANNER_NAME = "static-scanner"
SKILL_VETTER_NAME = "skill-vetter"

LEGACY_CODE_SCANNER_NAME = "skill-code-scanner"
LEGACY_STATIC_SCANNER_NAME = "cisco-static-scanner"

DEFAULT_BUILTIN_SCANNERS = [CODE_SCANNER_NAME, STATIC_SCANNER_NAME]

_ALIASES = {
    LEGACY_CODE_SCANNER_NAME: CODE_SCANNER_NAME,
    LEGACY_STATIC_SCANNER_NAME: STATIC_SCANNER_NAME,
}


def canonicalize_scanner_name(name: str) -> str:
    """Return the public stable scanner name for *name*."""
    return _ALIASES.get(name, name)


def scanner_aliases_for(name: str) -> set[str]:
    """Return all accepted names for a canonical scanner name."""
    canonical = canonicalize_scanner_name(name)
    aliases = {canonical}
    aliases.update(alias for alias, target in _ALIASES.items() if target == canonical)
    return aliases
