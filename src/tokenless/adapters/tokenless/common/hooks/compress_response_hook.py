#!/usr/bin/env python3
"""Tokenless response compression hook with optional TOON encoding.

Reads a PostToolUse JSON from stdin, compresses the tool response
via ``tokenless compress-response``, then optionally re-encodes to TOON
format via ``tokenless compress-toon`` for additional token savings.

Runtime detection and Cosh-NG compatibility:
    The hook auto-detects Cosh-NG by checking whether tool_response is a
    ``{llmContent, returnDisplay}`` wrapper (see cosh-ng hook.rs::wrap_tool_response).
    When running under Cosh-NG:
    - Extracts and compresses only ``llmContent`` (model-visible content).
    - Emits a ``replacement`` field (introduced by issue #1614) so Cosh-NG
      replaces the original response with the compressed version.
    - Keeps environment/error attribution in ``additionalContext``.
    - Uses ``cosh-ng`` as the agent ID for stats attribution.
    - Detects unsupported Cosh-NG versions and fails open with compression
      disabled rather than falling back to duplicate summary injection.

Pipeline: Env Attribution -> Layered dispatch -> Compression -> TOON Encoding
  1. If tool_response contains errors, classify as environment vs logic issue
     and inject "Skip retry" guidance for LLM
  2. 3-layer tool dispatch:
     - Content retrieval (Read/Glob/Grep) -> skip all compression
     - Shell/exec (Bash/Shell) -> moderate truncation (64K strings)
     - Other tools -> zero-truncation compress-response + TOON
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
    build_cosh_ng_post_tool_output,
    classify_env_error,
    cosh_ng_supports_replacement,
    extract_llm_content,
    get_thresholds,
    is_cosh_ng_runtime,
    is_skill_file,
    resolve_binary,
    resolve_tool_call_id,
    skip,
    try_parse_json,
    unwrap_string_json,
    warn,
)

# -- constants ---------------------------------------------------------------

_FALLBACK_AGENT_ID = os.environ.get("TOKENLESS_AGENT_ID", "tokenless")
# Cosh-NG gets a distinct agent ID for stats attribution.
_COSH_NG_AGENT_ID = "cosh-ng"
_MIN_RESPONSE_CHARS = 200


# -- helpers -------------------------------------------------------------------


def _resolve_agent_id(is_cosh_ng: bool) -> str:
    """Return the appropriate agent ID for stats attribution."""
    if is_cosh_ng:
        return _COSH_NG_AGENT_ID
    return _FALLBACK_AGENT_ID


def _build_additional_context(
    content: str,
    env_attribution: str = "",
) -> str:
    parts = []
    if env_attribution:
        parts.append(env_attribution)
    parts.append(content)
    return "\n".join(parts)


def _warn_subprocess(label: str, proc: subprocess.CompletedProcess) -> None:
    """Log a non-zero subprocess exit with truncated stderr."""
    detail = (proc.stderr or "").strip()[:200]
    warn(
        f"{label} exited {proc.returncode}: {detail}"
        if detail
        else f"{label} exited {proc.returncode} with empty stderr"
    )


# -- main --------------------------------------------------------------------


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

    # 3. Detect Cosh-NG runtime
    cosh_ng = is_cosh_ng_runtime(input_data)
    agent_id = _resolve_agent_id(cosh_ng)

    # 4. Cosh-NG version guard: fail open if replacement not supported.
    # When Cosh-NG doesn't support the replacement field, compression
    # would inject duplicate content (original + compressed summary).
    if cosh_ng and not cosh_ng_supports_replacement():
        warn(
            "Cosh-NG version does not support response replacement. "
            "Compression disabled to avoid duplicate injection."
        )
        skip()

    # 5. Extract tool_name
    tool_name = input_data.get("tool_name", "unknown")

    # 6. Extract tool_response -- Cosh-NG path: extract llmContent only.
    tool_response_raw = input_data.get("tool_response", "")
    if not tool_response_raw or tool_response_raw == "{}":
        skip()

    if cosh_ng:
        # Cosh-NG wraps tool_response as {llmContent, returnDisplay}.
        # We only compress llmContent (model-visible content).
        llm_content = extract_llm_content(input_data)
        if llm_content is None:
            skip()
        # For Cosh-NG, the response to compress is the llmContent string.
        tool_response_str = llm_content
        # Check if it's JSON for the compression pipeline
        parsed = try_parse_json(tool_response_str)
        if parsed is None:
            # Plain text llmContent -- wrap for compression pipeline
            tool_response = json.dumps(
                {"stdout": tool_response_str}, separators=(",", ":"),
                ensure_ascii=False,
            )
            parsed = try_parse_json(tool_response)
        else:
            tool_response = tool_response_str
    else:
        # Copilot-Shell path: tool_response is a plain string.
        if isinstance(tool_response_raw, str):
            # Skip skill files (YAML frontmatter)
            if is_skill_file(tool_response_raw):
                skip()
            unwrapped = unwrap_string_json(tool_response_raw)
            if not unwrapped:
                skip()  # Plain text, not JSON
            tool_response = unwrapped
        elif isinstance(tool_response_raw, (dict, list)):
            tool_response = json.dumps(tool_response_raw, separators=(",", ":"))
        else:
            skip()

        # Validate it's JSON
        parsed = try_parse_json(tool_response)
        if parsed is None:
            skip()

    # 7. Extract caller context
    session_id = input_data.get("session_id", "")
    tool_use_id = resolve_tool_call_id(agent_id, input_data)

    # 8. Environment attribution analysis
    env_attribution = ""
    attr_category, attr_fix_hint = classify_env_error(parsed)
    if attr_category:
        env_attribution = (
            f"[tokenless:env] {tool_name} failed: "
            f"{attr_category} ({attr_fix_hint}). Skip retry."
        )

    # 9. Content retrieval -- skip entirely (preserve integrity)
    if tool_name in SKIP_TOOLS:
        if env_attribution:
            if cosh_ng:
                output = build_cosh_ng_post_tool_output(
                    replacement=None,
                    additional_context=env_attribution,
                )
            else:
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

    # 10. All other tools -- skip small responses, but still inject
    # env attribution for error cases.
    if len(tool_response) < _MIN_RESPONSE_CHARS:
        if env_attribution:
            if cosh_ng:
                output = build_cosh_ng_post_tool_output(
                    replacement=None,
                    additional_context=env_attribution,
                )
            else:
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

    # 11. Step 1: Response compression with 3-layer thresholds
    compressed = tool_response
    used_resp_compression = False

    if isinstance(parsed, (dict, list)):
        thresholds = get_thresholds(tool_name)
        cmd = [
            tokenless_bin, "compress-response",
            "--agent-id", agent_id,
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

    # 12. Step 2: TOON encoding
    toon_output = ""

    if tokenless_bin:
        toon_parsed = try_parse_json(compressed)
        if toon_parsed is not None:
            toon_cmd = [tokenless_bin, "compress-toon", "--agent-id", agent_id]
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

    # 13. Compression skip check: if no savings achieved, don't emit replacement.
    if not used_resp_compression and not toon_output:
        # No compression savings -- only emit if there's env attribution.
        if env_attribution:
            if cosh_ng:
                output = build_cosh_ng_post_tool_output(
                    replacement=None,
                    additional_context=env_attribution,
                )
            else:
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

    # 14. Build response -- Cosh-NG vs Copilot-Shell paths diverge here.
    if cosh_ng:
        # Cosh-NG: emit replacement (model-visible compressed content) +
        # additionalContext (environment attribution).
        # The replacement field tells Cosh-NG to substitute the original
        # llmContent with the compressed version.
        additional_ctx = env_attribution if env_attribution else None
        output = build_cosh_ng_post_tool_output(
            replacement=final_output,
            additional_context=additional_ctx,
        )
    else:
        # Copilot-Shell: emit suppressOutput + additionalContext with
        # both the compressed content and environment attribution.
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
