#!/usr/bin/env python3
"""Cosh hook for response compression with optional TOON encoding.

Reads a cosh PostToolUse JSON from stdin, compresses the tool response
via ``tokenless compress-response``, then optionally re-encodes to TOON
format via ``toon -e`` for additional token savings.

Pipeline: Response Compression -> TOON Encoding (if JSON)
  1. Strip debug fields, nulls, empty values; truncate long strings/arrays
  2. If the compressed result is still valid JSON, encode to TOON format
  3. Stats are recorded automatically by tokenless compress-response.

Hook point: **PostToolUse**

This script is intentionally self-contained — it does NOT import any
tokenless package.  All it needs is the standard library and the
tokenless/toon binaries on $PATH.
"""

import json
import os
import shutil
import subprocess
import sys

# -- constants ---------------------------------------------------------------

_AGENT_ID = "copilot-shell"
_MIN_RESPONSE_LEN = 200

# Tools that return content the agent explicitly requested — must not compress.
_SKIP_TOOLS = {
    "Read", "read_file", "Glob", "list_directory",
    "NotebookRead", "read", "glob", "notebookread",
}

_TOKENLESS_FALLBACK = "/usr/bin/tokenless"
_TOON_FALLBACK = "/usr/libexec/tokenless/toon"


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


def _build_additional_context(
    tool_name: str, savings_pct: int, savings_label: str, content: str,
) -> str:
    return (
        f"[tokenless] {tool_name} → {savings_label} ({savings_pct}% savings)\n"
        f"{content}"
    )


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