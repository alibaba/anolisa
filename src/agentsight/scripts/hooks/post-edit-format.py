#!/usr/bin/env python3
"""Post-edit auto-format hook for AI agents.

Reads edited file paths from stdin (one per line) and runs the
appropriate formatter. Failures are silently ignored so the agent
workflow is never blocked.

Supported formatters:
  .rs          -> cargo fmt -- <file>
  .py          -> ruff format <file>  (falls back to black)
  .ts/.tsx     -> npx prettier --write <file>
"""

import json
import os
import shutil
import subprocess
import sys


def format_rust(path: str) -> None:
    # rustfmt directly — works on any .rs file without crate context
    if shutil.which("rustfmt"):
        subprocess.run(
            ["rustfmt", path],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=30,
        )


def format_python(path: str) -> None:
    if shutil.which("ruff"):
        subprocess.run(
            ["ruff", "format", path],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=30,
        )
    elif shutil.which("black"):
        subprocess.run(
            ["black", "--quiet", path],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=30,
        )


def format_typescript(path: str) -> None:
    if shutil.which("npx"):
        subprocess.run(
            ["npx", "prettier", "--write", path],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=30,
        )


FORMATTERS = {
    ".rs": format_rust,
    ".py": format_python,
    ".ts": format_typescript,
    ".tsx": format_typescript,
}


def main() -> None:
    raw = sys.stdin.read().strip()
    if not raw:
        return

    # Codex passes tool-use result as JSON; extract file paths.
    paths: list[str] = []
    try:
        data = json.loads(raw)
        if isinstance(data, dict):
            # Edit/Write: {"file_path": "..."}
            if "file_path" in data:
                paths.append(data["file_path"])
            # MultiEdit: {"edits": [{"file_path": "..."}, ...]}
            for edit in data.get("edits", []):
                if isinstance(edit, dict) and "file_path" in edit:
                    paths.append(edit["file_path"])
        elif isinstance(data, list):
            for item in data:
                if isinstance(item, dict) and "file_path" in item:
                    paths.append(item["file_path"])
    except (json.JSONDecodeError, TypeError):
        # Fallback: treat each line as a file path
        paths = [line.strip() for line in raw.splitlines() if line.strip()]

    for path in paths:
        if not os.path.isfile(path):
            continue
        ext = os.path.splitext(path)[1].lower()
        formatter = FORMATTERS.get(ext)
        if formatter:
            try:
                formatter(path)
            except Exception:
                pass


if __name__ == "__main__":
    main()
