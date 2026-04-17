"""Configuration loading for skill-ledger (``~/.config/skill-ledger/config.json``)."""

import json
from pathlib import Path
from typing import Any

from agent_sec_cli.skill_ledger.errors import ConfigError
from agent_sec_cli.skill_ledger.paths import get_config_dir

_DEFAULT_CONFIG: dict[str, Any] = {
    "signingBackend": "ed25519",
    "skillDirs": [],
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
    - Scalar / list top-level keys: user value wins outright.
    - ``scanners`` (list[dict]): merge by ``name`` — user entries override
      defaults with the same ``name``; defaults not in user are preserved.
    - ``parsers`` (dict[str, dict]): shallow dict merge per parser name.
    """
    merged = dict(defaults)
    for key, user_val in user.items():
        if key == "scanners" and isinstance(user_val, list):
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
    - ``"path/*"`` — glob pattern: each matching subdirectory is a skill
    - ``"path/to/skill"`` — single skill directory
    """
    if config is None:
        config = load_config()

    skill_dirs: list[Path] = []
    for entry in config.get("skillDirs", []):
        entry = str(entry)
        expanded = Path(entry).expanduser()

        if entry.endswith("/*"):
            # Glob mode: parent directory, each subdirectory is a skill
            parent = expanded.parent
            if parent.is_dir():
                for child in sorted(parent.iterdir()):
                    if child.is_dir() and not child.name.startswith("."):
                        skill_dirs.append(child)
        else:
            # Single directory
            if expanded.is_dir():
                skill_dirs.append(expanded)

    return skill_dirs


def save_config(config: dict[str, Any]) -> Path:
    """Write *config* to ``config.json``.  Creates parent dirs if needed."""
    path = config_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(config, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    return path
