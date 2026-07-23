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

Output contract per agent:
  - cosh-ng: the compressed payload replaces the original llmContent via
    ``hookSpecificOutput.replacement`` (introduced by issue #1614).
    ``additionalContext`` carries environment attribution.
  - claude-code (>= 2.1.121): the compressed payload *replaces* the
    model-visible tool result via ``hookSpecificOutput.updatedToolOutput``.
    ``additionalContext`` is additive in Claude Code (appended alongside
    the original tool result), so it only carries genuinely additive
    diagnostics (environment attribution). Older Claude Code versions fail
    open: compression is disabled instead of injecting a duplicate payload
    (issue #1645).
  - other agents: the compressed payload is injected via
    ``additionalContext`` per each runtime's hook contract.

The agent ID is read from the TOKENLESS_AGENT_ID environment variable
(set by the install action script).  Fallback paths follow the ANOLISA
FHS spec: /usr/bin/tokenless.
"""

from __future__ import annotations

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
    parse_version,
    resolve_binary,
    resolve_tool_call_id,
    secure_write_text,
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

# Claude Code added hookSpecificOutput.updatedToolOutput (normal-path tool
# output replacement for all tools) in v2.1.121. Older versions only support
# the additive additionalContext, which would duplicate the payload.
_CLAUDE_AGENT_ID = "claude-code"
_CLAUDE_MIN_REPLACE_VERSION = (2, 1, 121)

# Cache for `claude --version`, keyed on binary path+mtime+size so upgrades
# invalidate it. Hooks run as a fresh process per tool call and spawning the
# node CLI every time would add noticeable latency.
_CLAUDE_VERSION_CACHE = os.path.join(
    os.path.expanduser("~"), ".tokenless", ".claude-version"
)


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


def _emit(output: dict) -> None:
    print(json.dumps(output, ensure_ascii=False))


def _emit_attribution_or_skip(
    env_attribution: str,
    cosh_ng: bool = False,
) -> None:
    """Pass the original result through, keeping only additive diagnostics.

    Emits an attribution-only output when present (it is genuinely
    additive and safe on every agent), otherwise a plain skip.
    For Cosh-NG, uses the replacement-aware output format.
    Never returns.
    """
    if env_attribution:
        if cosh_ng:
            _emit(build_cosh_ng_post_tool_output(
                replacement=None,
                additional_context=env_attribution,
            ))
        else:
            _emit({
                "suppressOutput": True,
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": env_attribution,
                },
            })
        sys.exit(0)
    skip()


def _cached_claude_version(claude_bin: str) -> tuple | None:
    """Return the Claude Code version tuple, caching `claude --version`."""
    try:
        st = os.stat(claude_bin)
        cache_key = f"{claude_bin}:{int(st.st_mtime)}:{st.st_size}"
    except OSError:
        cache_key = claude_bin

    try:
        with open(_CLAUDE_VERSION_CACHE) as f:
            key, _, ver_str = f.read().strip().partition("\n")
        if key == cache_key:
            return parse_version(ver_str)
    except OSError:
        pass

    try:
        proc = subprocess.run(
            [claude_bin, "--version"],
            capture_output=True, text=True, timeout=5,
        )
    except Exception as e:
        warn(f"claude --version failed: {e}")
        return None
    if proc.returncode != 0:
        return None
    ver = parse_version(proc.stdout)
    if ver:
        try:
            # Same hardened write as other ~/.tokenless state files (0o600,
            # symlink-safe) so the cache stays private on shared HOMEs.
            secure_write_text(
                _CLAUDE_VERSION_CACHE, f"{cache_key}\n{proc.stdout.strip()}"
            )
        except OSError:
            pass
    return ver


def _claude_supports_replacement() -> bool:
    """Whether the running Claude Code supports updatedToolOutput (>= 2.1.121).

    Returns False when the version cannot be determined; the caller then
    fails open by disabling compression, so unknown versions never receive a
    duplicate compressed payload through additionalContext.
    """
    claude_bin = resolve_binary("claude")
    if not claude_bin:
        return False
    ver = _cached_claude_version(claude_bin)
    return ver is not None and ver >= _CLAUDE_MIN_REPLACE_VERSION


def _restore_dropped_schema_fields(original: dict, compressed: dict) -> dict:
    """Restore top-level keys dropped by compression when originally empty.

    compress-response drops nulls, empty values ("" / {} / []) and configured
    debug fields. Built-in Claude Code tools expect a stable output schema
    (e.g. Bash: stdout/stderr/interrupted/isImage), so cheap empty fields are
    restored for updatedToolOutput; intentionally dropped non-empty debug
    payloads stay dropped.
    """
    restored = dict(compressed)
    for key, value in original.items():
        if key in restored:
            continue
        if value is None or value == "" or value == {} or value == []:
            restored[key] = value
    return restored


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
    # When Cosh-NG doesn't support the replacement field (version too old
    # or version info unavailable), compression would inject duplicate
    # content (original + compressed summary). Conservatively disable.
    if cosh_ng and not cosh_ng_supports_replacement():
        warn(
            "Cosh-NG version does not support response replacement "
            "(version too old or not configured). "
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
        else:
            tool_response = tool_response_str
    else:
        # Copilot-Shell and other runtimes: tool_response is a string.
        if not isinstance(tool_response_raw, str):
            tool_response_raw = json.dumps(tool_response_raw, ensure_ascii=False)
        tool_response_str = tool_response_raw
        parsed = try_parse_json(tool_response_str)
        if parsed is None:
            tool_response = tool_response_str
        else:
            tool_response = tool_response_str

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
        _emit_attribution_or_skip(env_attribution, cosh_ng=cosh_ng)

    # 10. All other tools -- skip small responses, but still inject
    # env attribution for error cases.
    if len(tool_response) < _MIN_RESPONSE_CHARS:
        _emit_attribution_or_skip(env_attribution, cosh_ng=cosh_ng)

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

    # Nothing shrank -- pass the original through untouched instead of
    # emitting a same-size duplicate of the response.
    if not used_resp_compression and not toon_output:
        _emit_attribution_or_skip(env_attribution, cosh_ng=cosh_ng)

    # 13. Build response -- runtime-specific output format.
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
        _emit(output)
        return

    # Claude Code: additionalContext is *additive* -- the model would see both
    # the original tool result and the compressed copy, inflating the context
    # instead of shrinking it (issue #1645). Replace the tool result via
    # updatedToolOutput (>= 2.1.121) and keep additionalContext for additive
    # diagnostics only. Unsupported versions fail open via pass-through.
    if agent_id == _CLAUDE_AGENT_ID:
        if not _claude_supports_replacement():
            warn(
                "Claude Code < 2.1.121 (or version unknown): "
                "updatedToolOutput unsupported, response compression disabled."
            )
            _emit_attribution_or_skip(env_attribution, cosh_ng=cosh_ng)

        if isinstance(tool_response_raw, (dict, list)):
            # Structured original: the replacement must preserve the built-in
            # tool output schema, so TOON (a text encoding) is not applicable
            # and only a genuine compress-response win qualifies.
            if not used_resp_compression:
                _emit_attribution_or_skip(env_attribution, cosh_ng=cosh_ng)
            compressed_parsed = try_parse_json(compressed)
            if isinstance(tool_response_raw, dict) and isinstance(
                compressed_parsed, dict
            ):
                updated_output = _restore_dropped_schema_fields(
                    tool_response_raw, compressed_parsed
                )
            elif compressed_parsed is not None:
                updated_output = compressed_parsed
            else:
                _emit_attribution_or_skip(env_attribution, cosh_ng=cosh_ng)
            # Restoring empty schema fields can cancel out a marginal win;
            # only replace when the result is strictly smaller than the
            # original serialized response.
            if len(json.dumps(updated_output, separators=(",", ":"))) >= len(
                tool_response
            ):
                _emit_attribution_or_skip(env_attribution, cosh_ng=cosh_ng)
        else:
            # String original (JSON-in-string): replace with the smallest
            # text form (TOON when it won, compressed JSON otherwise).
            updated_output = final_output

        hook_output = {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": updated_output,
        }
        if env_attribution:
            hook_output["additionalContext"] = env_attribution
        _emit({"suppressOutput": True, "hookSpecificOutput": hook_output})
        return

    # Other agents: inject via additionalContext per their hook contracts.
    context = _build_additional_context(
        final_output,
        env_attribution=env_attribution,
    )

    _emit({
        "suppressOutput": True,
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": context,
        },
    })


if __name__ == "__main__":
    main()
