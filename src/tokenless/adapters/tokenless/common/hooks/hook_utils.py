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

# -- Unified tool categorization ----------------------------------------------

# Tool categories are loaded from tool_categories.json, which serves as the
# single source of truth for both tool-ready and compression strategies.
# This fixes inconsistencies like Grep being classified as Shell in some places
# and Read in others.

_TOOL_CATEGORIES_PATH = os.path.join(os.path.dirname(__file__), "tool_categories.json")

# Hardcoded fallback sets — used only when tool_categories.json is missing or
# invalid. Matches the minimum safe classification from before the JSON was
# introduced, ensuring content-retrieval tools are never accidentally compressed.
_FALLBACK_SKIP_TOOLS = [
    "Read", "read", "read_file", "read_many_files",
    "Glob", "glob", "list_directory",
    "Grep", "grep", "grep_search", "search_files",
    "Lsp", "lsp",
    "NotebookRead", "notebook_read", "notebookread",
]
_FALLBACK_SHELL_TOOLS = [
    "Bash", "bash", "Shell", "shell", "exec", "terminal",
    "run_shell_command", "execute_command", "process",
]


def _load_tool_categories() -> dict:
    """Load tool categories from unified JSON file with validation."""
    try:
        with open(_TOOL_CATEGORIES_PATH, "r") as f:
            data = json.load(f)

        # Validate required structure
        required_layers = ["layer_1_skip", "layer_2_shell", "layer_3_api"]
        for layer in required_layers:
            if layer not in data:
                raise ValueError(f"Missing required layer: {layer}")
            if not isinstance(data[layer], dict):
                raise ValueError(f"Layer {layer} must be a dict")

        # layer_1 and layer_2 require a "tools" list; layer_3 is implicit
        for layer in ("layer_1_skip", "layer_2_shell"):
            if "tools" not in data[layer]:
                raise ValueError(f"Layer {layer} missing 'tools' field")
            if not isinstance(data[layer]["tools"], list):
                raise ValueError(f"Layer {layer}.tools must be a list")

        return data
    except (FileNotFoundError, json.JSONDecodeError, ValueError) as e:
        print(f"Warning: Failed to load tool_categories.json: {e}", file=sys.stderr)
        # Fallback to hardcoded safe sets so content-retrieval tools are
        # never accidentally compressed even if the JSON is unavailable.
        return {
            "layer_1_skip": {"tools": list(_FALLBACK_SKIP_TOOLS)},
            "layer_2_shell": {"tools": list(_FALLBACK_SHELL_TOOLS)},
            "layer_3_api": {},
        }


_tool_categories = _load_tool_categories()

# 3-layer Compression strategy:
#   Layer 1: Content retrieval (Read/Glob/Grep) → skip all compression
#   Layer 2: Shell/exec (Bash/Shell/exec) → moderate truncation (64K strings)
#   Layer 3: API/structured (all other) → zero-truncation cleanup (1M strings)

# Layer 1: Content retrieval tools — skip all compression (preserve integrity).
# These tools return file content or search results that should not be truncated.
SKIP_TOOLS: set[str] = set(_tool_categories.get("layer_1_skip", {}).get("tools", []))

# Layer 2: Shell/exec tools (moderate truncation).
# These tools produce text output that can be safely truncated if too long.
SHELL_TOOLS: set[str] = set(_tool_categories.get("layer_2_shell", {}).get("tools", []))

# Layer 3: API tools (zero-truncation).
# These tools return structured data or API responses that should not be truncated.
# No explicit set needed; tools not in SKIP_TOOLS or SHELL_TOOLS are Layer 3.

# Thresholds are read from tool_categories.json (single source of truth).
# Hardcoded fallbacks match the JSON defaults; used only if the JSON field
# is missing or the file failed to load.

# Layer 2 thresholds: moderate truncation for shell/exec output.
# Restores old ResponseCompressor defaults for shell commands (git log, ls,
# cat, etc.) where truncation is acceptable.
# 64K strings: 95% of real shell output (git diff ~63K, git log ~34K) preserved.
# 128 arrays: 95% of result sets (test results, audit reports) preserved.
_layer2_thr = _tool_categories.get("layer_2_shell", {}).get("thresholds", {})
_SHELL_TRUNCATE_STRINGS_AT = _layer2_thr.get("truncate_strings_at", 65_536)
_SHELL_TRUNCATE_ARRAYS_AT = _layer2_thr.get("truncate_arrays_at", 128)
_SHELL_MAX_DEPTH = _layer2_thr.get("max_depth", 8)

# Layer 3 thresholds: zero-truncation for API/structured data.
_layer3_thr = _tool_categories.get("layer_3_api", {}).get("thresholds", {})
_TRUNCATE_STRINGS_AT = _layer3_thr.get("truncate_strings_at", 1_048_576)
_TRUNCATE_ARRAYS_AT = _layer3_thr.get("truncate_arrays_at", 65_536)
_MAX_DEPTH = _layer3_thr.get("max_depth", 32)

# Backward-compatible alias — direct reference (not a copy) so consumers see
# the same set as SKIP_TOOLS. Used by compress_toon_hook.py for the standalone
# TOON-only path where "content retrieval" is the more descriptive name.
CONTENT_RETRIEVAL_TOOLS = SKIP_TOOLS


def get_thresholds(tool_name: str) -> tuple[int, int, int]:
    """Return (truncate_strings_at, truncate_arrays_at, max_depth) for a tool.

    Layer 2 (shell/exec) tools use moderate truncation; all others use
    Layer 3 zero-truncation thresholds. Single dispatch point used by all
    adapters (codex, hermes, openclaw, compress_response_hook).
    """
    if tool_name in SHELL_TOOLS:
        return (_SHELL_TRUNCATE_STRINGS_AT, _SHELL_TRUNCATE_ARRAYS_AT, _SHELL_MAX_DEPTH)
    return (_TRUNCATE_STRINGS_AT, _TRUNCATE_ARRAYS_AT, _MAX_DEPTH)


# -- Shared environment error patterns ----------------------------------------
#
# Superset of patterns from both the codex and hook adapters. Uses regex
# matching (case-insensitive) so patterns like "/bin/sh:.*: not found" work
# correctly. Both codex/scripts/compress-response and compress_response_hook
# import this list and the classify_env_error() function below.

ENV_PATTERNS: list[tuple[list[str], str, str]] = [
    (
        [
            "command not found",
            "not installed",
            "which: no",
            r"No command\s",
            "cannot execute",
            "is not recognized",
            "Could not find",
            "unable to locate",
            "Package not found",
            r"/bin/sh:.*: not found",
            "command not found:",
        ],
        "ENV_DEPENDENCY_MISSING",
        "Missing dependency detected. Install it or ask the user for guidance.",
    ),
    (
        [
            "Permission denied",
            "permission denied",
            "Operation not permitted",
            "EACCES",
            "Access denied",
            r"cannot open .* for writing",
        ],
        "ENV_PERMISSION",
        "Permission denied. Check file/directory permissions or run with appropriate access.",
    ),
    (
        [
            "No such file or directory",
            "ENOENT",
            "cannot find",
            "File not found",
            "does not exist",
        ],
        "ENV_FILE_MISSING",
        "Required file or directory not found. Verify the path or create it.",
    ),
    (
        [
            "Connection refused",
            "Could not resolve host",
            "Network is unreachable",
            r"curl: \(7\)",
            r"curl: \(6\)",
            "Failed to connect",
            "Name or service not known",
            "Couldn't resolve host",
            "Temporary failure in name resolution",
            "ECONNREFUSED",
            "ETIMEDOUT",
            "Connection timed out",
        ],
        "ENV_NETWORK",
        "Network connectivity issue. Check DNS, proxy, and firewall settings.",
    ),
    (
        [
            "ModuleNotFoundError",
            "ImportError",
            "No module named",
            "cannot import name",
            "npm ERR! 404",
        ],
        "ENV_PACKAGE_MISSING",
        "Required package or module is missing. Install the needed dependency.",
    ),
]


def classify_env_error(tool_response) -> tuple[str | None, str | None]:
    """Detect environment errors in tool output.

    Accepts either a parsed dict (with stderr/error/exit_code fields) or a
    plain string. Returns (category_tag, fix_hint) or (None, None).

    Shared by codex/scripts/compress-response and compress_response_hook.
    """
    if isinstance(tool_response, dict):
        text = str(tool_response.get("stderr", "")) + str(tool_response.get("error", ""))
        # Use `is None` — `or` would treat exit_code=0 (success) as falsy and
        # incorrectly fall through to exitCode.
        exit_code = tool_response.get("exit_code")
        if exit_code is None:
            exit_code = tool_response.get("exitCode")
        if exit_code is not None and exit_code == 0 and not text:
            return None, None
    elif isinstance(tool_response, str):
        text = tool_response
    else:
        return None, None

    if not text:
        return None, None

    for patterns, category, hint in ENV_PATTERNS:
        for pat in patterns:
            if re.search(pat, text, re.IGNORECASE):
                return category, hint

    return None, None


# -- Context file for rewrite session tracking --

_CONTEXT_DIR = os.path.join(os.path.expanduser("~"), ".tokenless")
_CONTEXT_FILE = os.path.join(_CONTEXT_DIR, ".rewrite-context")

# -- Binary resolution (cached) -----------------------------------------------

_resolved_cache: dict[tuple, str | None] = {}


def resolve_binary(name: str, *fallback_paths: str) -> str | None:
    """Locate a binary by PATH search, then optional fallback paths.

    Results are cached per (name, fallback_paths) — different callers passing
    distinct fallback paths for the same name get independent cache entries.
    """
    cache_key = (name, fallback_paths)
    if cache_key in _resolved_cache:
        return _resolved_cache[cache_key]

    result: str | None = None
    path = shutil.which(name)
    if path:
        result = path
    else:
        for fp in fallback_paths:
            if fp and os.path.isfile(fp) and os.access(fp, os.X_OK):
                result = fp
                break

    _resolved_cache[cache_key] = result
    return result


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


def resolve_tool_call_id(agent_id: str, input_data: dict) -> str:
    """Resolve the tool call identifier to record for a hook invocation.

    Qwen Code's serialized hook input carries two identifiers: the internal
    ``tool_use_id`` (``toolu_<ts>_<rand>``, always present) and
    ``tool_call_id`` (the LLM provider's original call ID, e.g. ``call_xxx``,
    snake_case — Qwen Code's TS uses camelCase ``toolCallId`` internally but
    normalizes to ``tool_call_id`` on the wire). For the qwencode agent,
    prefer the provider call ID and fall back to the internal one; for other
    agents, keep the existing priority (``tool_use_id`` first).
    """
    if agent_id == "qwencode":
        return (
            input_data.get("tool_call_id")
            or input_data.get("toolCallId")
            or input_data.get("tool_use_id", "")
        )
    return input_data.get("tool_use_id") or input_data.get("toolCallId", "")


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
