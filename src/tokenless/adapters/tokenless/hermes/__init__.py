"""Token-Less Plugin for Hermes Agent.

Combines multiple context-compression strategies into a single plugin:

  1. **Response compression** — ``transform_tool_result`` : compresses tool
     results via ``tokenless compress-response`` with per-layer thresholds
     (moderate for shell, zero-truncation for API tools), stripping debug
     fields, nulls, empty values.
  2. **TOON encoding** — ``transform_tool_result`` : pipeline step after
     response compression; re-encodes JSON results to TOON format via
     ``tokenless compress-toon`` for additional token savings (15-40%)
     with proper stats recording and size check.
  3. **Tool Ready** — ``pre_tool_call`` : environment readiness pre-check
     with auto-fix and skip-retry feedback for missing dependencies.
  4. **Command rewriting** — ``pre_tool_call`` : blocks shell commands
     and suggests RTK-rewritten equivalents.  Hermes's hook cannot modify
     arguments, so the agent must re-execute with the suggested command
     (one extra round-trip).  Safe: ``rtk rewrite`` only does text
     substitution, never executes the command.
  5. **Session tracking** — ``on_session_start`` : propagates agent/session
     IDs to tokenless stats recording.

Not available in Hermes: schema compression (Hermes hooks do not expose
tool schemas).

Every hook degrades gracefully: if ``tokenless`` is not installed, all
hooks are silently skipped.

Activation is controlled by the Hermes plugin system — list ``tokenless`` in
``plugins.enabled`` in ``config.yaml``, or enable via
``hermes plugins enable tokenless``.
"""

from __future__ import annotations

import json
import logging
import os
import subprocess
import sys
from typing import Any

# Import shared FHS constants from hook_utils.
# realpath needed because install.sh symlinks __init__.py into
# ~/.hermes/plugins/, and plain __file__ points to the symlink
# path — resolving .. from there hits a nonexistent directory.
sys.path.insert(0, os.path.join(os.path.dirname(os.path.realpath(__file__)), "..", "common", "hooks"))
from hook_utils import (
    _TOKENLESS_FALLBACK,
    _TOKENLESS_LOCAL_SHARE,
    _TOKENLESS_LOCAL_LIB,
    _RTK_FALLBACK,
    _RTK_LOCAL_SHARE,
    _RTK_LOCAL_LIB,
    resolve_binary,
    warn as _warn_shared,
    try_parse_json as _try_parse_json,
    is_skill_file as _is_skill_file,
    write_context as _write_context,
    run as _run,
    parse_version as _parse_version,
    SKIP_TOOLS as _SKIP_TOOLS_SHARED,
    SHELL_TOOLS as _SHELL_TOOLS_SHARED,
    get_thresholds,
)

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

AGENT_ID = "hermes-agent"
_MIN_RESPONSE_LEN = 200

_SKIP_TOOLS: set[str] = _SKIP_TOOLS_SHARED | {
    "session_search", "list_sessions",
}

# Use shared SHELL_TOOLS directly - all tools (including "terminal") are now
# defined in the unified tool_categories.json
_SHELL_TOOLS: set[str] = _SHELL_TOOLS_SHARED

_MIN_RTK_VERSION = (0, 35, 0)

# ---------------------------------------------------------------------------
# Binary resolution (thin wrapper over shared cached resolve_binary)
# ---------------------------------------------------------------------------

# Hermes-specific fallback paths for the RTK binary.
_RTK_LIB_FALLBACK = "/usr/lib/anolisa/tokenless/rtk"


def _resolve_binary(name: str, fallback: str) -> str | None:
    """Resolve binary with hermes-specific fallback paths (cached via shared)."""
    local_bin = os.path.join(os.path.expanduser("~"), ".local", "bin", name)
    if name == "rtk":
        return resolve_binary(name, fallback, _RTK_LIB_FALLBACK, local_bin, _RTK_LOCAL_LIB, _RTK_LOCAL_SHARE)
    return resolve_binary(name, fallback, local_bin, _TOKENLESS_LOCAL_LIB, _TOKENLESS_LOCAL_SHARE)


def _have(name: str, fallback: str) -> bool:
    return _resolve_binary(name, fallback) is not None


# ---------------------------------------------------------------------------
# Helpers (shared via hook_utils)
# ---------------------------------------------------------------------------


# ---------------------------------------------------------------------------
# 1. Response Compression (via tokenless compress-response)
# ---------------------------------------------------------------------------


def _compress_response(
    tool_name: str,
    result: str,
    session_id: str,
    tool_call_id: str,
) -> str | None:
    tokenless_bin = _resolve_binary("tokenless", _TOKENLESS_FALLBACK)
    if not tokenless_bin:
        return None

    parsed = _try_parse_json(result)
    if not isinstance(parsed, (dict, list)):
        return None

    # 3-layer dispatch: shell tools use moderate truncation, API tools use zero-truncation
    thresholds = get_thresholds(tool_name)

    cmd = [
        tokenless_bin, "compress-response",
        "--agent-id", AGENT_ID,
        "--truncate-strings-at", str(thresholds[0]),
        "--truncate-arrays-at", str(thresholds[1]),
        "--max-depth", str(thresholds[2]),
    ]
    if session_id:
        cmd.extend(["--session-id", session_id])
    if tool_call_id:
        cmd.extend(["--tool-use-id", tool_call_id])

    proc = _run(cmd, result)
    if not proc or proc.returncode != 0 or not proc.stdout.strip():
        return None

    compressed = proc.stdout.strip()
    if len(compressed) >= len(result):
        return None
    return compressed


# ---------------------------------------------------------------------------
# 2. TOON Encoding (via tokenless compress-toon)
# ---------------------------------------------------------------------------


def _encode_toon(data: str, session_id: str = "", tool_call_id: str = "") -> tuple[str, int] | None:
    tokenless_bin = _resolve_binary("tokenless", _TOKENLESS_FALLBACK)
    if not tokenless_bin:
        return None

    parsed = _try_parse_json(data)
    if not isinstance(parsed, (dict, list)):
        return None

    cmd = [tokenless_bin, "compress-toon", "--agent-id", AGENT_ID]
    if session_id:
        cmd.extend(["--session-id", session_id])
    if tool_call_id:
        cmd.extend(["--tool-use-id", tool_call_id])

    proc = _run(cmd, data, timeout=1)
    if not proc or proc.returncode != 0 or not proc.stdout.strip():
        return None

    toon_text = proc.stdout.strip()
    if len(toon_text) >= len(data):
        return None

    savings_pct = 0
    if len(data) > 0:
        savings_pct = (len(data) - len(toon_text)) * 100 // len(data)

    return toon_text, savings_pct


# ---------------------------------------------------------------------------
# 3. Tool Ready (via tokenless env-check)
# ---------------------------------------------------------------------------


def _env_check(tool_name: str) -> str | None:
    """Run tool-ready env-check and return feedback if tool is not ready."""
    tokenless_bin = _resolve_binary("tokenless", _TOKENLESS_FALLBACK)
    if not tokenless_bin:
        return None

    proc = _run([tokenless_bin, "env-check", "--tool", tool_name, "--json"], "", timeout=5)
    if not proc or not proc.stdout.strip():
        return None

    try:
        parsed = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return None

    status = parsed.get("status", "UNKNOWN")
    if status in ("UNKNOWN", "READY"):
        return None

    # Attempt auto-fix
    proc = _run([tokenless_bin, "env-check", "--tool", tool_name, "--fix", "--json"], "", timeout=10)
    if not proc or not proc.stdout.strip():
        return _not_ready_msg(tool_name)

    try:
        fix_parsed = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return _not_ready_msg(tool_name)

    if fix_parsed.get("status") == "READY":
        return None

    diagnostic = fix_parsed.get("diagnostic", "")
    return diagnostic or _not_ready_msg(tool_name)


def _not_ready_msg(tool_name: str) -> str:
    return f"[tokenless:ready] {tool_name}: NOT_READY. Skip retry."


# ---------------------------------------------------------------------------
# 4. Command Rewriting (via rtk rewrite)
# ---------------------------------------------------------------------------


def _try_rewrite(
    args: Any,
    session_id: str,
    tool_call_id: str,
) -> dict[str, str] | None:
    """Attempt RTK command rewrite for terminal tool calls.

    Calls ``rtk rewrite <command>`` — a pure text substitution that never
    executes the command.  On success, returns a block directive suggesting
    the rewritten command so the agent re-executes with the optimized version.
    """
    rtk_bin = _resolve_binary("rtk", _RTK_FALLBACK)
    if not rtk_bin:
        return None

    if not isinstance(args, dict):
        return None

    command = args.get("command", "")
    if not command:
        return None

    # Version guard — non-fatal
    try:
        ver_proc = subprocess.run(
            [rtk_bin, "--version"], capture_output=True, text=True, timeout=3,
        )
        ver = _parse_version(ver_proc.stdout)
        if ver and ver < _MIN_RTK_VERSION:
            logger.warning("tokenless: rtk %s too old (need >= 0.35.0), rewrite skipped", ver_proc.stdout.strip())
            return None
    except Exception:
        pass

    # Write context file so rtk (running as proxy later) can recover IDs
    _write_context(AGENT_ID, session_id, tool_call_id)

    # Set env vars for rtk stats context
    env = os.environ.copy()
    env["TOKENLESS_AGENT_ID"] = AGENT_ID
    if session_id:
        env["TOKENLESS_SESSION_ID"] = session_id
    if tool_call_id:
        env["TOKENLESS_TOOL_USE_ID"] = tool_call_id

    proc = subprocess.run(
        [rtk_bin, "rewrite", command],
        capture_output=True, text=True, timeout=5, env=env,
    )

    # Exit code protocol (from rtk rewrite_cmd.rs):
    #   0 = rewrite available, Allow verdict (auto-allow by permission rule)
    #   1 = no RTK equivalent (passthrough)
    #   2 = deny rule matched (let Hermes handle)
    #   3 = Ask/Default verdict (rewrite available but permission model requires
    #       user confirmation; in non-interactive hook context, treat as valid
    #       rewrite since the intent is token optimization, not permission gating)
    if proc.returncode == 1 or proc.returncode == 2:
        return None
    if proc.returncode != 0 and proc.returncode != 3:
        return None

    rewritten = proc.stdout.strip()
    if not rewritten or rewritten == command:
        return None

    logger.info("tokenless: rtk rewrite %s → %s", command, rewritten)
    return {
        "action": "block",
        "message": f"[tokenless:rewrite] Re-execute as: {rewritten}",
    }


# ---------------------------------------------------------------------------
# Hook callbacks
# ---------------------------------------------------------------------------


def on_session_start(**kwargs: Any) -> None:
    """Record session mapping for stats context."""
    session_id = kwargs.get("session_id", "")
    if session_id:
        os.environ["TOKENLESS_SESSION_ID"] = str(session_id)
        logger.debug("tokenless: session_start session_id=%s", session_id)


def on_pre_tool_call(
    tool_name: str = "",
    args: Any = None,
    task_id: str = "",
    session_id: str = "",
    tool_call_id: str = "",
    **kwargs: Any,
) -> dict[str, str] | None:
    """Tool Ready + RTK rewrite pre-check.

    Step 1: env-check blocks when the tool's environment is not ready.
    Step 2: for ``terminal`` calls, blocks and suggests RTK-rewritten
    command (one extra round-trip; safe — rtk rewrite never executes).
    """
    # Step 1: env-check (all tools, needs tokenless)
    if _have("tokenless", _TOKENLESS_FALLBACK):
        if session_id:
            os.environ["TOKENLESS_SESSION_ID"] = str(session_id)
        feedback = _env_check(tool_name)
        if feedback:
            logger.info("tokenless: tool-ready blocking %s — %s", tool_name, feedback)
            return {"action": "block", "message": feedback}

    # Step 2: RTK rewrite (terminal only, needs rtk)
    if tool_name in _SHELL_TOOLS and _have("rtk", _RTK_FALLBACK):
        result = _try_rewrite(args, str(session_id), str(tool_call_id))
        if result:
            return result

    return None


def on_transform_tool_result(
    tool_name: str = "",
    args: Any = None,
    result: str = "",
    task_id: str = "",
    session_id: str = "",
    tool_call_id: str = "",
    duration_ms: int = 0,
    **kwargs: Any,
) -> str | None:
    """Response compression + TOON encoding pipeline.

    Replaces the tool result string with a compressed/TOON-encoded version.
    Runs after post_tool_call; first valid string return wins.

    Content retrieval tools (Read/Glob/Grep) are skipped entirely.
    Shell/exec tools (Bash/Shell) use moderate truncation (64K/128/8).
    All other tools use zero-truncation compress-response + TOON.
    """
    if not _have("tokenless", _TOKENLESS_FALLBACK):
        return None

    # Content retrieval — skip entirely (preserve integrity)
    if tool_name in _SKIP_TOOLS:
        return None

    if not result or result in ("{}", "[]"):
        return None

    # Skip skill files (YAML frontmatter)
    if _is_skill_file(result):
        return None

    # Skip small responses
    if len(result) < _MIN_RESPONSE_LEN:
        return None

    # Validate it's JSON
    parsed = _try_parse_json(result)
    if parsed is None:
        return None

    # Normalize: result is already a JSON string (Hermes tool contract)
    original = result
    original_len = len(original)

    # Step 1: Response compression (per-layer thresholds via get_thresholds)
    compressed = _compress_response(tool_name, result,
                                     str(session_id), str(tool_call_id))
    current = compressed if compressed else result

    # Step 2: TOON encoding
    toon_result = _encode_toon(current, str(session_id), str(tool_call_id))
    used_compression = compressed is not None
    used_toon = toon_result is not None

    if not used_compression and not used_toon:
        return None

    # Build final output
    if used_toon:
        toon_text, savings_pct = toon_result
        final = toon_text
        final_len = len(toon_text)
        savings_label = (
            "response compressed + TOON encoded"
            if used_compression
            else "TOON encoded"
        )
    else:
        final = current  # type: ignore[assignment]
        final_len = len(final)
        savings_pct = (original_len - final_len) * 100 // original_len if original_len else 0
        savings_label = "response compressed"

    logger.info(
        "tokenless: %s %s: %d -> %d chars (%d%% reduction)",
        savings_label, tool_name, original_len, final_len, savings_pct,
    )

    return final


# ---------------------------------------------------------------------------
# Plugin entry point
# ---------------------------------------------------------------------------


def register(ctx: Any) -> None:
    """Register all tokenless hooks with the Hermes plugin system."""

    ctx.register_hook("on_session_start", on_session_start)
    ctx.register_hook("pre_tool_call", on_pre_tool_call)
    ctx.register_hook("transform_tool_result", on_transform_tool_result)

    # Log what's active
    features: list[str] = []
    if _have("tokenless", _TOKENLESS_FALLBACK):
        features.append("response-compression")
        features.append("toon-encoding")
        features.append("tool-ready")
    if _have("rtk", _RTK_FALLBACK):
        features.append("rtk-rewrite")

    logger.info(
        "tokenless: Hermes plugin registered — active features: %s",
        ", ".join(features) if features else "none (install tokenless/rtk binary)",
    )
