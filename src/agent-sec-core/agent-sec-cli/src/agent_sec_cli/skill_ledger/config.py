"""Configuration loading for skill-ledger (``~/.config/agent-sec/skill-ledger/config.json``)."""

import json
import logging
from pathlib import Path
from typing import Any

from agent_sec_cli.skill_ledger.errors import ConfigError
from agent_sec_cli.skill_ledger.paths import get_config_dir

logger = logging.getLogger(__name__)

_SKILL_MANIFEST = "SKILL.md"

_DEFAULT_CONFIG: dict[str, Any] = {
    "signingBackend": "ed25519",
    "skillDirs": [
        "~/.openclaw/skills/*",
        "~/.copilot-shell/skills/*",
        "/usr/share/anolisa/skills/*",
    ],
    # ── Scanner / parser registry (see design doc §2) ──
    "scanners": [
        {
            "name": "skill-vetter",
            "type": "skill",
            "parser": "findings-array",
            "description": "LLM-driven 4-phase skill audit",
        },
    ],
    "parsers": {
        "findings-array": {
            "type": "findings-array",
        },
    },
}


def config_path() -> Path:
    """Return the path to ``config.json``."""
    return get_config_dir() / "config.json"


def _deep_merge_config(
    defaults: dict[str, Any], user: dict[str, Any]
) -> dict[str, Any]:
    """Merge *user* config onto *defaults* with list-of-dict awareness.

    Rules:
    - ``skillDirs`` (list[str]): **additive** — user entries are appended
      to defaults; duplicates are removed while preserving order.
    - ``scanners`` (list[dict]): merge by ``name`` — user entries override
      defaults with the same ``name``; defaults not in user are preserved.
    - ``parsers`` (dict[str, dict]): shallow dict merge per parser name.
    - Other scalar / list top-level keys: user value wins outright.
    """
    merged = dict(defaults)
    for key, user_val in user.items():
        if key == "skillDirs" and isinstance(user_val, list):
            # Additive: defaults + user, dedup preserving order
            seen: set[str] = set()
            combined: list[str] = []
            for entry in [*defaults.get("skillDirs", []), *user_val]:
                entry_str = str(entry)
                if entry_str not in seen:
                    seen.add(entry_str)
                    combined.append(entry_str)
            merged["skillDirs"] = combined
        elif key == "scanners" and isinstance(user_val, list):
            # Index defaults by name for O(1) lookup
            by_name: dict[str, dict[str, Any]] = {}
            for s in defaults.get("scanners", []):
                if isinstance(s, dict) and "name" in s:
                    by_name[s["name"]] = s
            # User entries override by name
            for s in user_val:
                if isinstance(s, dict) and "name" in s:
                    by_name[s["name"]] = s
            merged["scanners"] = list(by_name.values())
        elif key == "parsers" and isinstance(user_val, dict):
            merged_parsers = dict(defaults.get("parsers", {}))
            merged_parsers.update(user_val)
            merged["parsers"] = merged_parsers
        else:
            merged[key] = user_val
    return merged


def load_config() -> dict[str, Any]:
    """Load and return the config file.  Returns defaults if the file does not exist."""
    path = config_path()
    if not path.is_file():
        return dict(_DEFAULT_CONFIG)
    try:
        raw = path.read_text(encoding="utf-8")
        cfg = json.loads(raw)
        if not isinstance(cfg, dict):
            raise ConfigError(
                f"config.json must be a JSON object, got {type(cfg).__name__}"
            )
        return _deep_merge_config(_DEFAULT_CONFIG, cfg)
    except json.JSONDecodeError as exc:
        raise ConfigError(f"Invalid JSON in {path}: {exc}") from exc


def resolve_skill_dirs(config: dict[str, Any] | None = None) -> list[Path]:
    """Expand ``skillDirs`` entries (glob + single-dir) into concrete directories.

    Supports two formats per entry:
    - ``"path/*"`` — glob pattern: each matching subdirectory **that contains
      SKILL.md** is included.
    - ``"path/to/skill"`` — single skill directory; must also contain
      ``SKILL.md`` to be included.

    Non-existent directories are silently skipped.  Duplicates (by resolved
    path) are removed while preserving discovery order.
    """
    if config is None:
        config = load_config()

    skill_dirs: list[Path] = []
    seen: set[Path] = set()

    for entry in config.get("skillDirs", []):
        entry = str(entry)
        expanded = Path(entry).expanduser()

        if entry.endswith("/*"):
            # Glob mode: parent directory, each child with SKILL.md is a skill
            parent = expanded.parent
            if parent.is_dir():
                for child in sorted(parent.iterdir()):
                    if (
                        child.is_dir()
                        and not child.name.startswith(".")
                        and (child / _SKILL_MANIFEST).is_file()
                    ):
                        resolved = child.resolve()
                        if resolved not in seen:
                            seen.add(resolved)
                            skill_dirs.append(child)
        else:
            # Single directory — still requires SKILL.md
            if expanded.is_dir() and (expanded / _SKILL_MANIFEST).is_file():
                resolved = expanded.resolve()
                if resolved not in seen:
                    seen.add(resolved)
                    skill_dirs.append(expanded)

    return skill_dirs


# ---------------------------------------------------------------------------
# Auto-remember: append unknown skill dirs on check/certify
# ---------------------------------------------------------------------------


def _compact_skill_dirs(entries: list[str]) -> list[str]:
    """Remove entries that are subsumed by a glob in the same list.

    A specific path ``parent/X`` is redundant when ``parent/*`` also appears.
    Preserves order; keeps the glob, drops the specifics.
    """
    glob_parents: set[str] = set()
    for entry in entries:
        if entry.endswith("/*"):
            # Normalise: resolve ~ so "/home/user/.copilot-shell/skills/*"
            # and "~/.copilot-shell/skills/*" are treated as the same parent.
            glob_parents.add(str(Path(entry[:-2]).expanduser().resolve()))

    compacted: list[str] = []
    seen: set[str] = set()
    for entry in entries:
        if entry in seen:
            continue
        seen.add(entry)

        # Skip specific paths whose parent is covered by a glob
        if not entry.endswith("/*"):
            expanded = Path(entry).expanduser().resolve()
            parent_str = str(expanded.parent)
            if parent_str in glob_parents:
                continue

        compacted.append(entry)
    return compacted


def is_covered(skill_dir: Path, config: dict[str, Any] | None = None) -> bool:
    """Return ``True`` if *skill_dir* would be discovered by current config."""
    if config is None:
        config = load_config()
    resolved_target = skill_dir.resolve()
    all_dirs = resolve_skill_dirs(config)
    return any(d.resolve() == resolved_target for d in all_dirs)


def remember_skill_dir(
    skill_dir: Path, config: dict[str, Any] | None = None
) -> str | None:
    """Append *skill_dir* (or its parent glob) to ``skillDirs`` if not covered.

    Heuristic for entry format:
    - If the parent directory contains **at least two** sibling sub-directories
      that each contain ``SKILL.md``, add ``"parent/*"`` (glob pattern).
    - Otherwise, add the specific directory path.

    After appending, runs :func:`_compact_skill_dirs` to prune entries that
    are now subsumed by the new (or existing) glob.

    Returns the entry string that was added, or ``None`` if already covered.
    """
    if config is None:
        config = load_config()

    if is_covered(skill_dir, config):
        return None

    parent = skill_dir.parent
    sibling_skills = (
        [
            d
            for d in parent.iterdir()
            if d.is_dir()
            and not d.name.startswith(".")
            and (d / _SKILL_MANIFEST).is_file()
        ]
        if parent.is_dir()
        else []
    )

    if len(sibling_skills) >= 2:
        entry = str(parent) + "/*"
    else:
        entry = str(skill_dir)

    existing = list(config.get("skillDirs", []))
    if entry not in existing:
        existing.append(entry)
    config["skillDirs"] = _compact_skill_dirs(existing)
    save_config(config)
    logger.info("Added %r to skillDirs in %s", entry, config_path())

    return entry


def save_config(config: dict[str, Any]) -> Path:
    """Write *config* to ``config.json``.  Creates parent dirs if needed."""
    path = config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(config, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    return path
