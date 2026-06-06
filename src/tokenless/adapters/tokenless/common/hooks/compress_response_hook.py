#!/usr/bin/env python3
"""Tokenless response compression hook with optional TOON encoding.

Reads a PostToolUse JSON from stdin, compresses the tool response
via ``tokenless compress-response``, then optionally re-encodes to TOON
format via ``tokenless compress-toon`` for additional token savings.

Pipeline: Env Attribution → Skip-tool分流 → Response Compression → TOON Encoding
  1. If tool_response contains errors, classify as environment vs logic issue
     and inject "Skip retry" guidance for LLM
  2. For skip-tools (shell/search): emit attribution only (no compression);
     for other tools: proceed with full compression pipeline
  3. Strip debug fields, nulls, empty values; truncate long strings/arrays
  4. If the compressed result is still valid JSON, encode to TOON format
  5. Stats are recorded automatically by tokenless compress-response.

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

from hook_utils import resolve_binary, skip, warn, try_parse_json, unwrap_string_json, is_skill_file, SKIP_TOOLS, _TOKENLESS_FALLBACK, _TOKENLESS_LOCAL_SHARE, _TOKENLESS_LOCAL_LIB

# -- constants ---------------------------------------------------------------

_AGENT_ID = os.environ.get("TOKENLESS_AGENT_ID", "tokenless")
_MIN_RESPONSE_CHARS = 200


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

    has_error = bool(error_text) or (exit_code is not None and exit_code != 0)
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


def _warn_subprocess(label: str, proc: subprocess.CompletedProcess) -> None:
    """Log a non-zero subprocess exit with truncated stderr."""
    detail = (proc.stderr or "").strip()[:200]
    warn(
        f"{label} exited {proc.returncode}: {detail}"
        if detail
        else f"{label} exited {proc.returncode} with empty stderr"
    )


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

    # 3. Extract tool_name (skip-tools分流 handled after attribution)
    tool_name = input_data.get("tool_name", "unknown")

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

    # 7. Validate it's JSON (needed for attribution on skip-tools too)
    parsed = try_parse_json(tool_response)
    if parsed is None:
        skip()

    # 8. Extract caller context
    session_id = input_data.get("session_id", "")
    tool_use_id = input_data.get("tool_use_id") or input_data.get("toolCallId", "")

    # 9. Environment attribution analysis
    env_attribution = ""
    attr_category, attr_fix_hint = _classify_env_error(parsed if isinstance(parsed, dict) else {})
    if attr_category:
        env_attribution = (
            f"[tokenless:env] {tool_name} failed: "
            f"{attr_category} ({attr_fix_hint}). Skip retry."
        )

    # 10. Skip content-retrieval / shell tools — attribution only, no compression
    if tool_name in SKIP_TOOLS:
        if env_attribution:
            output = {
                "suppressOutput": True,
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": env_attribution,
                },
            }
            print(json.dumps(output, ensure_ascii=False))
            return
        skip()

    # 11. Skip small responses (only for compression path — attribution already handled)
    if len(tool_response) < _MIN_RESPONSE_CHARS:
        skip()

    # 12. Step 1: Response compression (only on JSON objects/arrays)
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
                capture_output=True, text=True, timeout=3,
            )
            if proc.returncode == 0 and proc.stdout.strip():
                candidate = proc.stdout.strip()
                if len(candidate) < len(tool_response):
                    compressed = candidate
                    used_resp_compression = True
            elif proc.returncode != 0:
                _warn_subprocess("compress-response", proc)
        except Exception as e:
            warn(f"Response compression error: {e}")

    # 13. Step 2: TOON encoding (via tokenless compress-toon for stats)
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
                    capture_output=True, text=True, timeout=1,
                )
                if proc.returncode == 0 and proc.stdout.strip():
                    candidate = proc.stdout.strip()
                    if len(candidate) < len(compressed):
                        toon_output = candidate
                elif proc.returncode != 0:
                    _warn_subprocess("compress-toon", proc)
            except Exception as e:
                warn(f"TOON encoding error: {e}")

    # Determine final output
    final_output = toon_output if toon_output else compressed

    # 14. Build response
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
