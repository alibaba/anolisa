"""Shared utilities for tokenless Python hooks."""

import json
import os
import re
import shutil
import subprocess
import sys

# -- FHS fallback paths (ANOLISA spec) ----------------------------------------

_TOKENLESS_FALLBACK = "/usr/bin/tokenless"
_TOKENLESS_LOCAL_SHARE = os.path.join(
    os.path.expanduser("~"), ".local", "share", "anolisa", "tokenless", "tokenless"
)
_TOKENLESS_LOCAL_LIB = os.path.join(
    os.path.expanduser("~"), ".local", "lib", "anolisa", "tokenless", "tokenless"
)
_RTK_FALLBACK = "/usr/libexec/anolisa/tokenless/rtk"
_RTK_LOCAL_SHARE = os.path.join(
    os.path.expanduser("~"), ".local", "share", "anolisa", "tokenless", "rtk"
)
_RTK_LOCAL_LIB = os.path.join(
    os.path.expanduser("~"), ".local", "lib", "anolisa", "tokenless", "rtk"
)

# -- Unified skip-tools set (PascalCase from Claude Code, snake_case from Hermes) --

SKIP_TOOLS: set[str] = {
    "Read", "read_file", "Glob", "list_directory",
    "NotebookRead", "notebook_read", "notebookread", "read", "glob",
    # Shell execution tools — stdout may contain full documents, must not truncate
    "Bash", "bash", "Shell", "run_shell_command", "terminal", "execute_command",
    # Search tools — stdout may contain large file contents, must not truncate
    "Grep", "grep",
}

# -- Context file for rewrite session tracking --

_CONTEXT_DIR = os.path.join(os.path.expanduser("~"), ".tokenless")
_CONTEXT_FILE = os.path.join(_CONTEXT_DIR, ".rewrite-context")


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


def skip_silent() -> None:
    """Exit silently with empty stdout (codex protocol: empty stdout = passthrough)."""
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


def write_context(agent_id: str, session_id: str, tool_use_id: str) -> None:
    """Write context file for rtk rewrite session tracking."""
    os.makedirs(_CONTEXT_DIR, mode=0o700, exist_ok=True)
    if os.path.islink(_CONTEXT_FILE):
        os.unlink(_CONTEXT_FILE)
    flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    fd = os.open(_CONTEXT_FILE, flags, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(f"{agent_id}\n")
        f.write(f"{session_id}\n")
        f.write(f"{tool_use_id}\n")


def forward_stderr(proc: subprocess.CompletedProcess) -> None:
    """Forward subprocess stderr on failure (non-zero exit) via warn()."""
    if proc.returncode != 0 and proc.stderr:
        warn(proc.stderr.rstrip())


def run(args: list[str], input_data: str, timeout: int = 3) -> subprocess.CompletedProcess | None:
    """Run a subprocess with input data, returning None on failure."""
    try:
        proc = subprocess.run(
            args,
            input=input_data,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
        forward_stderr(proc)
        return proc
    except Exception:
        return None


def parse_version(version_str: str) -> tuple | None:
    """Parse a version string like '0.35.0' into a (major, minor, patch) tuple."""
    m = re.search(r"(\d+)\.(\d+)\.(\d+)", version_str)
    if m:
        return (int(m.group(1)), int(m.group(2)), int(m.group(3)))
    return None
