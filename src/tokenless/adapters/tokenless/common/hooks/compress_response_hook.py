#!/usr/bin/env python3
"""Tokenless response compression hook for Cosh-NG, Claude Code, Qoder, and OpenCode.

Reads a PostToolUse JSON from stdin, compresses the tool response
via ``tokenless compress-response``, then optionally re-encodes to TOON
format via ``tokenless compress-toon`` for additional token savings.

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
  - claude-code (>= 2.1.121): the compressed payload *replaces* the
    model-visible tool result via ``hookSpecificOutput.updatedToolOutput``.
    ``additionalContext`` is additive in Claude Code (appended alongside
    the original tool result), so it only carries genuinely additive
    diagnostics (environment attribution). Older Claude Code versions fail
    open: compression is disabled instead of injecting a duplicate payload
    (issue #1645).
  - qoder-cli: the compressed payload replaces the response via the string
    field ``hookSpecificOutput.updatedToolOutput``. Structured responses are
    serialized as compact JSON because Qoder rejects object and array values.
    Qoder supports replacement for every tool, so compressed data is never
    appended beside the original.
  - opencode: the adapter translates ``updatedToolOutput`` to OpenCode's
    mutable ``tool.execute.after`` output. ``additionalContext`` remains
    reserved for additive readiness and environment diagnostics.
  - cosh-ng: the compressed payload replaces the response via
    ``hookSpecificOutput.updatedToolResponse``.  Extract only ``llmContent``
    from wrapped responses; never include ``returnDisplay``.  Keep
    environment/error attribution in ``additionalContext`` (additive).
    Unsupported Cosh-NG versions fail open with compression disabled.
  - other agents: the compressed payload is injected via
    ``additionalContext`` per each runtime's hook contract.

The agent ID is read from the TOKENLESS_AGENT_ID environment variable
(set by the install action script).  When running under Cosh-NG, the
agent ID is overridden to ``cosh-ng`` for correct stats attribution.
Fallback paths follow the ANOLISA FHS spec: /usr/bin/tokenless.
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
    classify_env_error,
    detect_cosh_ng_runtime,
    get_thresholds,
    is_skill_file,
    parse_version,
    resolve_agent_id,
    resolve_binary,
    resolve_tool_call_id,
    secure_write_text,
    skip,
    try_parse_json,
    unwrap_string_json,
    warn,
)

# -- constants ---------------------------------------------------------------

_MIN_RESPONSE_CHARS = 200

# Claude Code added hookSpecificOutput.updatedToolOutput (normal-path tool
# output replacement for all tools) in v2.1.121. Older versions only support
# the additive additionalContext, which would duplicate the payload.
_CLAUDE_AGENT_ID = "claude-code"
_CLAUDE_MIN_REPLACE_VERSION = (2, 1, 121)
_QODER_AGENT_ID = "qoder-cli"
_OPENCODE_AGENT_ID = "opencode"

# Cache for `claude --version`, keyed on binary path+mtime+size so upgrades
# invalidate it. Hooks run as a fresh process per tool call and spawning the
# node CLI every time would add noticeable latency.
_CLAUDE_VERSION_CACHE = os.path.join(
    os.path.expanduser("~"), ".tokenless", ".claude-version"
)


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


def _emit(output: dict) -> None:
    print(json.dumps(output, ensure_ascii=False))


def _emit_attribution_or_skip(env_attribution: str) -> None:
    """Pass the original result through, keeping only additive diagnostics.

    Emits an attribution-only additionalContext when present (it is genuinely
    additive and safe on every agent), otherwise a plain skip. Never returns.
    """
    if env_attribution:
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


def _build_replacement_output(
    tool_response_raw: object,
    tool_response: str,
    compressed: str,
    final_output: str,
    used_resp_compression: bool,
) -> tuple[bool, object]:
    """Build a schema-safe replacement for runtimes that support one."""
    if not isinstance(tool_response_raw, (dict, list)):
        return True, final_output

    # TOON text cannot replace a structured response without changing the
    # host tool schema, so this path requires a real JSON compression win.
    if not used_resp_compression:
        return False, None

    compressed_parsed = try_parse_json(compressed)
    if isinstance(tool_response_raw, dict) and isinstance(compressed_parsed, dict):
        updated_output = _restore_dropped_schema_fields(
            tool_response_raw, compressed_parsed
        )
    elif compressed_parsed is not None:
        updated_output = compressed_parsed
    else:
        return False, None

    # Restoring empty schema fields can cancel out a marginal win.
    serialized = json.dumps(updated_output, separators=(",", ":"))
    if len(serialized) >= len(tool_response):
        return False, None
    return True, updated_output


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
    # 1. Detect runtime (Cosh-NG vs copilot-shell)
    cosh_ng_version = detect_cosh_ng_runtime()
    cosh_ng_detected = cosh_ng_version is not None

    # If Cosh-NG is detected but unsupported version, fail open
    if cosh_ng_detected and cosh_ng_version == (0, 0, 0):
        warn("Unsupported Cosh-NG version. Response compression disabled (fail open).")
        skip()

    # 2. Resolve agent ID based on runtime
    agent_id = resolve_agent_id()

    # 3. Resolve binaries
    tokenless_bin = resolve_binary(
        "tokenless", _TOKENLESS_FALLBACK, _TOKENLESS_LOCAL_SHARE, _TOKENLESS_LOCAL_LIB
    )
    if not tokenless_bin:
        warn("tokenless is not installed. Response compression hook disabled.")
        skip()

    # 4. Read stdin JSON
    try:
        input_data = json.load(sys.stdin)
    except (json.JSONDecodeError, EOFError, ValueError):
        warn("failed to read PostToolUse payload. Passing through unchanged.")
        skip()

    # 5. Extract tool_name (skip-tools handled after attribution)
    tool_name = input_data.get("tool_name", "unknown")

    # 6. Extract tool_response
    tool_response_raw = input_data.get("tool_response", "")
    if not tool_response_raw or tool_response_raw == "{}":
        skip()

    # 7. For Cosh-NG, extract only llmContent from the wrapped response.
    #    Never include returnDisplay in the provider-visible replacement.
    llm_content = None
    if isinstance(tool_response_raw, dict):
        llm_content = tool_response_raw.get("llmContent")
        if llm_content is None:
            llm_content = tool_response_raw.get("returnDisplay")
    elif isinstance(tool_response_raw, str):
        # Try to parse as the {llmContent, returnDisplay} wrapper
        parsed_wrapper = try_parse_json(tool_response_raw)
        if isinstance(parsed_wrapper, dict) and "llmContent" in parsed_wrapper:
            llm_content = parsed_wrapper["llmContent"]

    # The model-visible content we will compress
    model_visible_before = llm_content if llm_content is not None else tool_response_raw

    # 8. Skip skill files (YAML frontmatter)
    if isinstance(model_visible_before, str) and is_skill_file(model_visible_before):
        skip()

    # 9. Normalize response
    if isinstance(model_visible_before, str):
        unwrapped = unwrap_string_json(model_visible_before)
        if not unwrapped:
            skip()  # Plain text, not JSON
        tool_response = unwrapped
    elif isinstance(model_visible_before, (dict, list)):
        tool_response = json.dumps(model_visible_before, separators=(",", ":"))
    else:
        skip()

    # 10. Validate it's JSON (needed for attribution on skip-tools too)
    parsed = try_parse_json(tool_response)
    if parsed is None:
        skip()

    # 11. Extract caller context
    session_id = input_data.get("session_id", "")
    tool_use_id = resolve_tool_call_id(agent_id, input_data)

    # 12. Environment attribution analysis
    env_attribution = ""
    attr_category, attr_fix_hint = classify_env_error(parsed)
    if attr_category:
        env_attribution = (
            f"[tokenless:env] {tool_name} failed: "
            f"{attr_category} ({attr_fix_hint}). Skip retry."
        )

    # 13. Content retrieval -- skip entirely (preserve integrity)
    if tool_name in SKIP_TOOLS:
        _emit_attribution_or_skip(env_attribution)

    # 14. All other tools -- skip small responses, but still inject
    # env attribution for error cases (small size doesn't mean the
    # error classification is unimportant to the agent).
    if len(tool_response) < _MIN_RESPONSE_CHARS:
        _emit_attribution_or_skip(env_attribution)

    # 15. Step 1: Response compression with 3-layer thresholds
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
                # Compare against actual model-visible before size
                if len(candidate) < len(tool_response):
                    compressed = candidate
                    used_resp_compression = True
            elif proc.returncode != 0:
                _warn_subprocess("compress-response", proc)
        except Exception as e:
            warn(f"Response compression error: {e}")

    # 16. Step 2: TOON encoding
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

    # Nothing shrank — pass the original through untouched instead of
    # emitting a same-size duplicate of the response (applies to all agents).
    if not used_resp_compression and not toon_output:
        _emit_attribution_or_skip(env_attribution)

    # 17. Build response — dispatch by agent runtime.
    #
    # Claude Code, Qoder, and OpenCode support real tool-output replacement. Keep
    # additionalContext for additive diagnostics only; using it for compressed
    # data would leave the original result in context and increase token use.
    if agent_id in {_CLAUDE_AGENT_ID, _QODER_AGENT_ID, _OPENCODE_AGENT_ID}:
        if agent_id == _CLAUDE_AGENT_ID and not _claude_supports_replacement():
            warn(
                "Claude Code < 2.1.121 (or version unknown): "
                "updatedToolOutput unsupported, response compression disabled."
            )
            _emit_attribution_or_skip(env_attribution)

        replace, updated_output = _build_replacement_output(
            tool_response_raw,
            tool_response,
            compressed,
            final_output,
            used_resp_compression,
        )
        if not replace:
            _emit_attribution_or_skip(env_attribution)

        # Qoder validates updatedToolOutput as a string even when the original
        # tool response is structured. Preserve the compact schema as JSON text.
        if agent_id == _QODER_AGENT_ID and not isinstance(updated_output, str):
            updated_output = json.dumps(
                updated_output, separators=(",", ":"), ensure_ascii=False
            )

        hook_output = {
            "hookEventName": "PostToolUse",
            "updatedToolOutput": updated_output,
        }
        if env_attribution:
            hook_output["additionalContext"] = env_attribution
        _emit({"suppressOutput": True, "hookSpecificOutput": hook_output})
        return

    # Cosh-NG: use updatedToolResponse for response replacement.
    # Skip compression if it doesn't reduce model-visible size.
    if cosh_ng_detected:
        if len(final_output) >= len(tool_response):
            _emit_attribution_or_skip(env_attribution)

        hook_specific = {
            "hookEventName": "PostToolUse",
            "updatedToolResponse": final_output,
        }
        if env_attribution:
            hook_specific["additionalContext"] = env_attribution
        _emit({"suppressOutput": True, "hookSpecificOutput": hook_specific})
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
