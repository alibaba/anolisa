#!/usr/bin/env python3
"""Tokenless response compression hook with optional TOON encoding.

Reads a PostToolUse JSON from stdin, compresses the tool response
via ``tokenless compress-response``, then optionally re-encodes to TOON
format via ``tokenless compress-toon`` for additional token savings.

Pipeline: Env Attribution → Response Compression → TOON Encoding
  1. If tool_response contains errors, classify as environment vs logic issue
     and inject "Skip retry" guidance for LLM
  2. Strip debug fields, nulls, empty values; truncate long strings/arrays
  3. If the compressed result is still valid JSON, encode to TOON format
  4. Stats are recorded automatically by tokenless compress-response.

Hook point: **PostToolUse**

The agent ID is read from the TOKENLESS_AGENT_ID environment variable
(set by the install action script).  Fallback paths follow the ANOLISA
FHS spec: /usr/bin/tokenless.
"""

import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hook_utils import resolve_binary, skip, warn, try_parse_json, unwrap_string_json, is_skill_file, _TOKENLESS_FALLBACK, _TOKENLESS_LOCAL_SHARE, _TOKENLESS_LOCAL_LIB

# -- constants ---------------------------------------------------------------

_AGENT_ID = os.environ.get("TOKENLESS_AGENT_ID", "tokenless")
_MIN_RESPONSE_CHARS = 200

_SKIP_TOOLS = {
    "Read", "read_file", "Glob", "list_directory",
    "NotebookRead", "read", "glob", "notebookread",
}


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
    """Extract the missing command name from shell error messages."""
    # bash: "bash: line 1: foo: command not found" or "foo: command not found"
    m = re.search(r": (\S+): command not found", error_text)
    if m:
        return m.group(1)
    # zsh: "command not found: foo"
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
    content: str,
    env_attribution: str = "",
) -> str:
    parts = []
    if env_attribution:
        parts.append(env_attribution)
    parts.append(content)
    return "\n".join(parts)


# -- main --------------------------------------------------------------------


def main() -> None:
    # 1. Resolve binaries
    tokenless_bin = resolve_binary("tokenless", _TOKENLESS_FALLBACK, _TOKENLESS_LOCAL_SHARE, _TOKENLESS_LOCAL_LIB)
    if not tokenless_bin:
        warn("tokenless is not installed. Response compression hook disabled.")
        skip()

    # 2. Read stdin JSON
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        warn("failed to read PostToolUse payload. Passing through unchanged.")
        skip()

    # 3. Skip content-retrieval tools
    tool_name = input_data.get("tool_name", "unknown")
    if tool_name in _SKIP_TOOLS:
        skip()

    # 4. Extract tool_response
    tool_response_raw = input_data.get("tool_response", "")
    if not tool_response_raw or tool_response_raw == "{}":
        skip()

    # 5. Skip skill files (YAML frontmatter)
    if isinstance(tool_response_raw, str) and is_skill_file(tool_response_raw):
        skip()

    # 6. Normalize response
    if isinstance(tool_response_raw, str):
        unwrapped = unwrap_string_json(tool_response_raw)
        if not unwrapped:
            skip()  # Plain text, not JSON
        tool_response = unwrapped
    elif isinstance(tool_response_raw, (dict, list)):
        tool_response = json.dumps(tool_response_raw, separators=(",", ":"))
    else:
        skip()

    # 7. Skip small responses (character count, not byte length)
    if len(tool_response) < _MIN_RESPONSE_CHARS:
        skip()

    # 8. Validate it's JSON
    parsed = try_parse_json(tool_response)
    if parsed is None:
        skip()

    # 9. Extract caller context
    session_id = input_data.get("session_id", "")
    tool_use_id = input_data.get("tool_use_id") or input_data.get("toolCallId", "")

    # 9b. Environment attribution analysis
    env_attribution = ""
    attr_category, attr_fix_hint = _classify_env_error(parsed if isinstance(parsed, dict) else {})
    if attr_category:
        env_attribution = (
            f"[tokenless:env] {tool_name} failed: "
            f"{attr_category} ({attr_fix_hint}). Skip retry."
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

    # 11. Step 2: TOON encoding (via tokenless compress-toon for stats)
    toon_output = ""

    if tokenless_bin:
        toon_parsed = try_parse_json(compressed)
        if toon_parsed is not None:
            toon_cmd = [tokenless_bin, "compress-toon", "--agent-id", _AGENT_ID]
            if session_id:
                toon_cmd.extend(["--session-id", session_id])
            if tool_use_id:
                toon_cmd.extend(["--tool-use-id", tool_use_id])
            try:
                proc = subprocess.run(
                    toon_cmd,
                    input=compressed,
                    capture_output=True, text=True, timeout=10,
                )
                if proc.returncode == 0 and proc.stdout.strip():
                    candidate = proc.stdout.strip()
                    if len(candidate) < len(compressed):
                        toon_output = candidate
            except Exception:
                pass

    # Determine final output
    final_output = toon_output if toon_output else compressed

    # 12. Build response
    context = _build_additional_context(
        final_output,
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
