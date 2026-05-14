#!/usr/bin/env python3
"""Tokenless response compression hook with optional TOON encoding.

Reads a PostToolUse JSON from stdin, compresses the tool response
via ``tokenless compress-response``, then optionally re-encodes to TOON
format via ``toon -e`` for additional token savings.

Pipeline: Env Attribution → Response Compression → TOON Encoding
  1. If tool_response contains errors, classify as environment vs logic issue
     and inject "Skip retry" guidance for LLM
  2. Strip debug fields, nulls, empty values; truncate long strings/arrays
  3. If the compressed result is still valid JSON, encode to TOON format
  4. Stats are recorded automatically by tokenless compress-response.

Hook point: **PostToolUse**

The agent ID is read from the TOKENLESS_AGENT_ID environment variable
(set by the install action script).  Fallback paths follow the ANOLISA
FHS spec: /usr/bin/tokenless, /usr/libexec/anolisa/tokenless/toon.
"""

import json
import os
import re
import shutil
import subprocess
import sys

# -- constants ---------------------------------------------------------------

_AGENT_ID = os.environ.get("TOKENLESS_AGENT_ID", "tokenless")
_MIN_RESPONSE_LEN = 200

# Tools that return content the agent explicitly requested — must not compress.
_SKIP_TOOLS = {
    "Read", "read_file", "Glob", "list_directory",
    "NotebookRead", "read", "glob", "notebookread",
}

_TOKENLESS_FALLBACK = "/usr/bin/tokenless"
_TOON_FALLBACK = "/usr/libexec/anolisa/tokenless/toon"


# -- helpers -----------------------------------------------------------------


def _resolve_binary(name: str, fallback_path: str) -> str | None:
    path = shutil.which(name)
    if path:
        return path
    if os.path.isfile(fallback_path) and os.access(fallback_path, os.X_OK):
        return fallback_path
    return None


def _skip() -> None:
    print(json.dumps({}))
    sys.exit(0)


def _warn(msg: str) -> None:
    print(f"[tokenless] WARNING: {msg}", file=sys.stderr)


def _try_parse_json(data: str) -> object | None:
    try:
        return json.loads(data)
    except (json.JSONDecodeError, ValueError):
        return None


def _unwrap_string_json(raw: str) -> str:
    """If raw is a JSON-encoded string whose inner content is valid JSON,
    unwrap it into the inner JSON object."""
    if not raw.startswith('"'):
        return raw
    inner = _try_parse_json(raw)
    if isinstance(inner, str):
        inner_obj = _try_parse_json(inner)
        if inner_obj is not None and isinstance(inner_obj, (dict, list)):
            return json.dumps(inner_obj, separators=(",", ":"))
        # Inner is plain text — not JSON, skip
        return ""
    return raw


def _is_skill_file(text: str) -> bool:
    """Detect YAML frontmatter markdown (skill files) that must not be compressed."""
    if not text.startswith("---"):
        return False
    lines = text.split("\n", 20)
    for line in lines[1:]:
        if line.startswith("name:") or line.startswith("description:"):
            return True
    return False


# -- env attribution patterns -------------------------------------------------

_ENV_PATTERNS: list[tuple[list[str], str, str]] = [
    # (patterns, category, fix_hint_template)
    (
        ["command not found", "not installed", "which: no", "No command '"],
        "ENV_DEPENDENCY_MISSING",
        "Install missing dependency: {missing}",
    ),
    (
        ["Permission denied", "permission denied", "Access denied"],
        "ENV_PERMISSION",
        "Check file/dir permissions or run with appropriate access",
    ),
    (
        ["No such file or directory", "cannot find", "does not exist", "ENOENT"],
        "ENV_FILE_MISSING",
        "Create or locate the required file/directory",
    ),
    (
        [
            "Connection refused", "ECONNREFUSED",
            "Connection timed out", "ETIMEDOUT",
            "curl: (7)", "curl: (6)", "network is unreachable",
        ],
        "ENV_NETWORK",
        "Check network connectivity and DNS resolution",
    ),
    (
        ["ModuleNotFoundError", "cannot find module", "ImportError", "npm ERR! 404"],
        "ENV_PACKAGE_MISSING",
        "Install the required module/package",
    ),
]


def _extract_missing_cmd(error_text: str) -> str:
    """Extract the missing command name from 'command not found' messages."""
    m = re.search(r"command not found: (\S+)", error_text)
    if m:
        return m.group(1)
    m = re.search(r"which: no (\S+)", error_text)
    if m:
        return m.group(1)
    return "unknown"


def _classify_env_error(parsed: dict) -> tuple[str | None, str | None]:
    """Classify tool execution failures as environment issues vs logic errors.

    Returns (category, fix_hint) if an environment error is detected, or
    (None, None) otherwise.
    """
    if not isinstance(parsed, dict):
        return None, None

    exit_code = parsed.get("exit_code")
    stderr_text = str(parsed.get("stderr", ""))
    error_field = str(parsed.get("error", ""))
    error_text = stderr_text + error_field

    has_error = bool(error_text) or exit_code in (1, 2)
    if not has_error:
        return None, None

    for patterns, category, fix_hint in _ENV_PATTERNS:
        for pat in patterns:
            if pat in error_text:
                if category == "ENV_DEPENDENCY_MISSING":
                    fix_hint = fix_hint.replace("{missing}", _extract_missing_cmd(error_text))
                return category, fix_hint

    return None, None


def _build_additional_context(
    tool_name: str, savings_pct: int, savings_label: str, content: str,
    env_attribution: str = "",
) -> str:
    parts = []
    if env_attribution:
        parts.append(env_attribution)
    parts.append(f"[tokenless] {tool_name} → {savings_label} ({savings_pct}% savings)")
    parts.append(content)
    return "\n".join(parts)


# -- main --------------------------------------------------------------------


def main() -> None:
    # 1. Resolve binaries
    tokenless_bin = _resolve_binary("tokenless", _TOKENLESS_FALLBACK)
    if not tokenless_bin:
        _warn("tokenless is not installed. Response compression hook disabled.")
        _skip()

    toon_bin = _resolve_binary("toon", _TOON_FALLBACK)

    # 2. Read stdin JSON
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        _warn("failed to read PostToolUse payload. Passing through unchanged.")
        _skip()

    # 3. Skip content-retrieval tools
    tool_name = input_data.get("tool_name", "unknown")
    if tool_name in _SKIP_TOOLS:
        _skip()

    # 4. Extract tool_response
    tool_response_raw = input_data.get("tool_response", "")
    if not tool_response_raw or tool_response_raw == "{}":
        _skip()

    # 5. Skip skill files (YAML frontmatter)
    if isinstance(tool_response_raw, str) and _is_skill_file(tool_response_raw):
        _skip()

    # 6. Normalize response
    if isinstance(tool_response_raw, str):
        # May be a JSON-encoded string wrapper or raw text
        unwrapped = _unwrap_string_json(tool_response_raw)
        if not unwrapped:
            _skip()  # Plain text, not JSON
        tool_response = unwrapped
    elif isinstance(tool_response_raw, (dict, list)):
        tool_response = json.dumps(tool_response_raw, separators=(",", ":"))
    else:
        _skip()

    # 7. Skip small responses
    if len(tool_response) < _MIN_RESPONSE_LEN:
        _skip()

    # 8. Validate it's JSON
    parsed = _try_parse_json(tool_response)
    if parsed is None:
        _skip()

    # 9. Extract caller context
    session_id = input_data.get("session_id", "")
    tool_use_id = input_data.get("tool_use_id") or input_data.get("toolCallId", "")

    # 9b. Environment attribution analysis
    env_attribution = ""
    attr_category, attr_fix_hint = _classify_env_error(parsed if isinstance(parsed, dict) else {})
    if attr_category:
        env_attribution = (
            f"[tokenless env-attribution] {tool_name} tool failed: "
            f"{attr_category} ({attr_fix_hint}). "
            f"Skip retry — this is an environment issue, not a logic error."
        )

    # 10. Step 1: Response compression (only on JSON objects/arrays)
    compressed = tool_response
    used_resp_compression = False

    if isinstance(parsed, (dict, list)):
        cmd = [tokenless_bin, "compress-response", "--agent-id", _AGENT_ID]
        if session_id:
            cmd.extend(["--session-id", session_id])
        if tool_use_id:
            cmd.extend(["--tool-use-id", tool_use_id])

        try:
            proc = subprocess.run(
                cmd,
                input=tool_response,
                capture_output=True, text=True, timeout=10,
            )
            if proc.returncode == 0 and proc.stdout.strip():
                compressed = proc.stdout.strip()
                used_resp_compression = True
        except Exception:
            pass  # Fall through to original

    # 11. Step 2: TOON encoding (if compressed result is valid JSON)
    toon_output = ""
    savings_label = ""

    if toon_bin:
        toon_parsed = _try_parse_json(compressed)
        if toon_parsed is not None:
            try:
                proc = subprocess.run(
                    [toon_bin, "-e"],
                    input=compressed,
                    capture_output=True, text=True, timeout=10,
                )
                if proc.returncode == 0 and proc.stdout.strip():
                    toon_output = proc.stdout.strip()
                    if used_resp_compression:
                        savings_label = "response compressed + TOON encoded"
                    else:
                        savings_label = "TOON encoded"
            except Exception:
                pass

    # Determine final label
    if not savings_label:
        if used_resp_compression:
            savings_label = "response compressed"
        else:
            savings_label = "passed through"

    # Determine final output and metrics
    if toon_output:
        final_output = toon_output
    else:
        final_output = compressed

    before_chars = len(tool_response)
    after_chars = len(final_output)

    savings_pct = 0
    if before_chars > 0:
        savings_pct = (before_chars - after_chars) * 100 // before_chars

    # 12. Build response
    context = _build_additional_context(
        tool_name, savings_pct, savings_label, final_output,
        env_attribution=env_attribution,
    )

    output = {
        "suppressOutput": True,
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": context,
        },
    }
    print(json.dumps(output, ensure_ascii=False))


if __name__ == "__main__":
    main()