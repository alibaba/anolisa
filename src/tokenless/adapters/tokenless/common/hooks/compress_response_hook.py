#!/usr/bin/env python3
"""Tokenless response compression hook with optional TOON encoding.

Reads a PostToolUse JSON from stdin, compresses the tool response
via ``tokenless compress-response``, then optionally re-encodes to TOON
format via ``tokenless compress-toon`` for additional token savings.

Pipeline: Env Attribution → Layered分流 → Compression → TOON Encoding
  1. If tool_response contains errors, classify as environment vs logic issue
     and inject "Skip retry" guidance for LLM
  2. 3-layer tool dispatch:
     - Content retrieval (Read/Glob/Grep) → skip all compression
     - Shell/exec (Bash/Shell) → moderate truncation (64K strings)
     - Other tools → zero-truncation compress-response + TOON
  3. Strip debug fields, nulls, empty values (no truncation risk)
  4. If the compressed result is still valid JSON, encode to TOON format
  5. Stats are recorded automatically by tokenless CLI commands.

Hook point: **PostToolUse**

The agent ID is read from the TOKENLESS_AGENT_ID environment variable
(set by the install action script).  Fallback paths follow the ANOLISA
FHS spec: /usr/bin/tokenless.
"""

import json
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from hook_utils import (
    _TOKENLESS_FALLBACK,
    _TOKENLESS_LOCAL_LIB,
    _TOKENLESS_LOCAL_SHARE,
    SKIP_TOOLS,
    classify_env_error,
    get_thresholds,
    is_skill_file,
    resolve_binary,
    resolve_tool_call_id,
    skip,
    try_parse_json,
    unwrap_string_json,
    warn,
)

# -- constants ---------------------------------------------------------------

_AGENT_ID = os.environ.get("TOKENLESS_AGENT_ID", "tokenless")
_MIN_RESPONSE_CHARS = 200


# -- helpers -------------------------------------------------------------------


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
    tokenless_bin = resolve_binary(
        "tokenless", _TOKENLESS_FALLBACK, _TOKENLESS_LOCAL_SHARE, _TOKENLESS_LOCAL_LIB
    )
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
    tool_use_id = resolve_tool_call_id(_AGENT_ID, input_data)

    # 9. Environment attribution analysis
    env_attribution = ""
    attr_category, attr_fix_hint = classify_env_error(parsed)
    if attr_category:
        env_attribution = (
            f"[tokenless:env] {tool_name} failed: "
            f"{attr_category} ({attr_fix_hint}). Skip retry."
        )

    # 10. Content retrieval — skip entirely (preserve integrity)
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

    # 11. All other tools — skip small responses, but still inject
    # env attribution for error cases (small size doesn't mean the
    # error classification is unimportant to the agent).
    if len(tool_response) < _MIN_RESPONSE_CHARS:
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

    # 12. Step 1: Response compression with 3-layer thresholds
    #   Layer 1 (content retrieval): already skipped above
    #   Layer 2 (shell/exec): moderate truncation (64K/128/8) — plain text output
    #   Layer 3 (API/structured): zero-truncation (1M/64K/32) — preserve content
    compressed = tool_response
    used_resp_compression = False

    if isinstance(parsed, (dict, list)):
        thresholds = get_thresholds(tool_name)
        cmd = [
            tokenless_bin, "compress-response",
            "--agent-id", _AGENT_ID,
            "--truncate-strings-at", str(thresholds[0]),
            "--truncate-arrays-at", str(thresholds[1]),
            "--max-depth", str(thresholds[2]),
        ]
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

    # 13. Step 2: TOON encoding
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
