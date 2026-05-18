"""Shared utilities for tokenless Python hooks."""

import json
import os
import shutil
import sys


def resolve_binary(name: str, *fallback_paths: str) -> str | None:
    path = shutil.which(name)
    if path:
        return path
    for fp in fallback_paths:
        if os.path.isfile(fp) and os.access(fp, os.X_OK):
            return fp
    return None


def skip() -> None:
    print(json.dumps({}))
    sys.exit(0)


def warn(msg: str) -> None:
    print(f"[tokenless] WARNING: {msg}", file=sys.stderr)


def try_parse_json(data: str) -> object | None:
    try:
        return json.loads(data)
    except (json.JSONDecodeError, ValueError):
        return None


def unwrap_string_json(raw: str) -> str | None:
    """If raw is a JSON-encoded string whose inner content is valid JSON,
    unwrap it into the inner JSON string. Returns None for plain text."""
    if not raw.startswith('"'):
        return raw
    inner = try_parse_json(raw)
    if isinstance(inner, str):
        inner_obj = try_parse_json(inner)
        if inner_obj is not None and isinstance(inner_obj, (dict, list)):
            return json.dumps(inner_obj, separators=(",", ":"))
        return None
    return raw


def is_skill_file(text: str) -> bool:
    """Detect YAML frontmatter markdown (skill files) that must not be compressed."""
    if not text.startswith("---"):
        return False
    lines = text.split("\n", 20)
    for line in lines[1:]:
        if line.startswith("name:") or line.startswith("description:"):
            return True
    return False
