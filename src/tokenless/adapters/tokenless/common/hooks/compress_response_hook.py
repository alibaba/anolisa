#!/usr/bin/env python3
"""Tokenless response compression hook for Cosh-NG, Claude Code, Qoder, and OpenCode.

Reads a PostToolUse JSON from stdin, forwards the model-visible tool
response to the unified ``tokenless compress`` entry point (protocol v1,
roadmap §5.4), and translates the CompressionResponse into the host's
envelope. JSON detection, tool threshold selection, TOON selection, and
final acceptance all live behind the entry point; this hook only parses the
host object, declares capabilities, and builds envelopes (§4.5).

One Tokenless subprocess per invocation (§5.6). Environment-error
attribution stays hook-side: it is genuinely additive diagnostics, not a
compression decision.

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
  - opencode: the adapter translates ``updatedToolOutput`` to OpenCode's
    mutable ``tool.execute.after`` output.
  - cosh-ng: the compressed payload replaces the response via
    ``hookSpecificOutput.updatedToolResponse``.  Extract only ``llmContent``
    from wrapped responses; never include ``returnDisplay``.  Unsupported
    Cosh-NG versions fail open with compression disabled.
  - other agents (additionalContext-only hosts): passthrough. Additive
    injection would append the compressed copy beside the still-visible
    original — a net token increase — so hosts without true output
    replacement remain passthrough (roadmap §7). Environment attribution is
    still injected: it is additive by design.

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
    SHELL_TOOLS,
    SKIP_TOOLS,
    build_compression_request,
    classify_env_error,
    detect_cosh_ng_runtime,
    is_skill_file,
    parse_version,
    resolve_agent_id,
    resolve_binary,
    resolve_tool_call_id,
    run_compress,
    secure_write_text,
    skip,
    try_parse_json,
    warn,
)

# -- constants ---------------------------------------------------------------

# Spawn-avoidance mirror of the entry point's 200-char gate. The authority
# lives in Rust; skipping here only saves the subprocess for content the
# entry would pass through anyway (normalization never grows the char
# count, so raw < 200 implies normalized < 200).
_MIN_RESPONSE_CHARS = 200

# Shell tool envelopes carry the log in one dominant text field. Unwrapping
# is worth a rebuilt envelope only when that field is large enough for the
# build/log engine to bite (its own gates start at 30 lines / 200 chars;
# 2000 chars keeps the rewrap machinery out of trivial outputs).
_SHELL_TEXT_FIELDS = ("stdout", "stderr")
_SHELL_UNWRAP_MIN_CHARS = 2_000

# Below the qwen/cosh extension manifests' 10 s host wrapper so a
# pathological input is killed here (fail-open skip) before the host kills
# the whole hook.
_COMPRESS_TIMEOUT = 8

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


def _shell_text_field(tool_name: str, envelope) -> tuple | None:
    """The dominant text field of a shell tool's envelope, or ``None``.

    Shell envelopes (``{"stdout": …, "stderr": …}``) are JSON to the entry
    point, which would compress them log-blind. Unwrapping the largest text
    field sends the log itself through the text slot; step 13 re-injects the
    compressed text into a same-shaped envelope, so the host's tool protocol
    is untouched (adapters own envelope knowledge, §4.5). Only the single
    largest field is compressed — one Tokenless subprocess per invocation
    (§5.6) — the other field stays byte-identical.
    """
    if tool_name not in SHELL_TOOLS or not isinstance(envelope, dict):
        return None
    best = None
    for name in _SHELL_TEXT_FIELDS:
        value = envelope.get(name)
        if (
            isinstance(value, str)
            and len(value) >= _SHELL_UNWRAP_MIN_CHARS
            and (best is None or len(value) > len(best[1]))
        ):
            best = (name, value)
    return best


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

    Returns False when the version cannot be determined; the hook then
    declares no replacement capability, so unknown versions never receive a
    duplicate compressed payload through additionalContext.
    """
    claude_bin = resolve_binary("claude")
    if not claude_bin:
        return False
    ver = _cached_claude_version(claude_bin)
    return ver is not None and ver >= _CLAUDE_MIN_REPLACE_VERSION


# -- main --------------------------------------------------------------------


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

    tool_name = input_data.get("tool_name", "unknown")
    tool_response_raw = input_data.get("tool_response", "")
    if not tool_response_raw or tool_response_raw == "{}":
        skip()

    # 5. For Cosh-NG, extract only llmContent from the wrapped response.
    #    Never include returnDisplay in the provider-visible replacement.
    llm_content = None
    if isinstance(tool_response_raw, dict):
        llm_content = tool_response_raw.get("llmContent")
        if llm_content is None:
            llm_content = tool_response_raw.get("returnDisplay")
    elif isinstance(tool_response_raw, str):
        parsed_wrapper = try_parse_json(tool_response_raw)
        if isinstance(parsed_wrapper, dict) and "llmContent" in parsed_wrapper:
            llm_content = parsed_wrapper["llmContent"]

    # The model-visible content we will send for compression
    model_visible_before = llm_content if llm_content is not None else tool_response_raw

    # 6. Skip skill files (YAML frontmatter). Spawn avoidance only: they are
    # never JSON, so the entry point would pass them through anyway.
    if isinstance(model_visible_before, str) and is_skill_file(model_visible_before):
        skip()

    # 7. Copy the model-visible value into the request content (§4.5). A
    # shell envelope's dominant text field goes through the text slot
    # instead of log-blind JSON; ensure_ascii=False matches the entry
    # point's normalization, so size gates measure Unicode characters on
    # both sides.
    shell_field = _shell_text_field(tool_name, model_visible_before)
    if shell_field is not None:
        content = shell_field[1]
    elif isinstance(model_visible_before, str):
        content = model_visible_before
    elif isinstance(model_visible_before, (dict, list)):
        content = json.dumps(
            model_visible_before, separators=(",", ":"), ensure_ascii=False
        )
    else:
        skip()

    # 8. Extract caller context
    session_id = input_data.get("session_id", "")
    tool_use_id = resolve_tool_call_id(agent_id, input_data)

    # 9. Environment attribution analysis — additive diagnostics, computed
    # hook-side. Only structured payloads are classified (with the same
    # string-unwrap the entry point applies): plain text never reached
    # attribution in the two-subprocess hook and still does not.
    if isinstance(model_visible_before, dict):
        attr_subject = model_visible_before
    else:
        parsed = try_parse_json(content)
        if isinstance(parsed, str):
            parsed = try_parse_json(parsed)
        attr_subject = parsed if isinstance(parsed, (dict, list)) else None
    env_attribution = ""
    attr_category, attr_fix_hint = classify_env_error(attr_subject)
    if attr_category:
        env_attribution = (
            f"[tokenless:env] {tool_name} failed: "
            f"{attr_category} ({attr_fix_hint}). Skip retry."
        )

    # 10. Capability declaration (§4.5): what can this host actually do?
    if cosh_ng_detected:
        can_replace = True
        replace_with_text = True  # updatedToolResponse accepts any text
    elif agent_id in {_QODER_AGENT_ID, _OPENCODE_AGENT_ID}:
        can_replace = True
        # An unwrapped shell field is plain text regardless of its envelope.
        replace_with_text = shell_field is not None or not isinstance(
            tool_response_raw, (dict, list)
        )
    elif agent_id == _CLAUDE_AGENT_ID:
        can_replace = _claude_supports_replacement()
        replace_with_text = shell_field is not None or not isinstance(
            tool_response_raw, (dict, list)
        )
        if not can_replace:
            warn(
                "Claude Code < 2.1.121 (or version unknown): "
                "updatedToolOutput unsupported, response compression disabled."
            )
    else:
        # additionalContext-only hosts have no true replacement: passthrough
        # (additive injection would duplicate the original — see module doc).
        can_replace = False
        replace_with_text = True

    # 11. Prefilters. Two of them only save the exec; SKIP_TOOLS does more
    # than that now. The entry point no longer keeps a tool list of its own
    # (roadmap §6.3), so until this hook declares a content origin per tool
    # (item 10) this check is the only thing keeping content retrieval — the
    # hottest PostToolUse traffic — away from the compressor. It is
    # load-bearing, not an optimization: do not drop it as one.
    if not can_replace:
        _emit_attribution_or_skip(env_attribution)
    if tool_name in SKIP_TOOLS:
        _emit_attribution_or_skip(env_attribution)
    if len(content) < _MIN_RESPONSE_CHARS:
        _emit_attribution_or_skip(env_attribution)

    # 12. The one Tokenless subprocess: the unified entry point decides.
    request = build_compression_request(
        content,
        agent_id,
        "post_tool",
        session_id=session_id,
        tool_use_id=tool_use_id,
        tool_name=tool_name,
        replace_output=True,
        publish_retrieve_tool=True,
        replace_with_text=replace_with_text,
    )
    response = run_compress(tokenless_bin, request, _COMPRESS_TIMEOUT)
    if response is None or response.get("disposition") != "applied":
        _emit_attribution_or_skip(env_attribution)

    output_text = response.get("output")
    if not isinstance(output_text, str) or not output_text:
        warn("tokenless compress returned no output. Passing through unchanged.")
        _emit_attribution_or_skip(env_attribution)

    # 13. Envelope construction — dispatch by agent runtime. An unwrapped
    # shell field is re-injected into a same-shaped envelope: the compressed
    # text replaces exactly the field that was sent, every other field stays
    # byte-identical.
    rewrapped = None
    if shell_field is not None:
        rewrapped = dict(model_visible_before)
        rewrapped[shell_field[0]] = output_text

    if cosh_ng_detected:
        hook_specific = {
            "hookEventName": "PostToolUse",
            "updatedToolResponse": rewrapped if rewrapped is not None else output_text,
        }
        if env_attribution:
            hook_specific["additionalContext"] = env_attribution
        _emit({"suppressOutput": True, "hookSpecificOutput": hook_specific})
        return

    if rewrapped is not None:
        updated_output = rewrapped
    elif replace_with_text:
        updated_output = output_text
    else:
        # Structured slot: the entry point guarantees schema-stable JSON for
        # an applied response. A parse failure means the subprocess boundary
        # was violated — fail open.
        updated_output = try_parse_json(output_text)
        if updated_output is None:
            warn("tokenless compress returned non-JSON for a structured slot.")
            _emit_attribution_or_skip(env_attribution)

    # Qoder validates updatedToolOutput as a string even when the original
    # tool response is structured. The entry point's compact serialization
    # is exactly that string; a rewrapped shell envelope serializes here.
    if agent_id == _QODER_AGENT_ID and not isinstance(updated_output, str):
        if rewrapped is not None:
            updated_output = json.dumps(
                rewrapped, separators=(",", ":"), ensure_ascii=False
            )
        else:
            updated_output = output_text

    hook_output = {
        "hookEventName": "PostToolUse",
        "updatedToolOutput": updated_output,
    }
    if env_attribution:
        hook_output["additionalContext"] = env_attribution
    _emit({"suppressOutput": True, "hookSpecificOutput": hook_output})


if __name__ == "__main__":
    main()
