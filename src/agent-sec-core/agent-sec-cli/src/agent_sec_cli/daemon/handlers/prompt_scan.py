"""Daemon handler for the scan-prompt CLI-compatible method."""

import asyncio
import json
from typing import Any

from agent_sec_cli.daemon.errors import BadRequestError
from agent_sec_cli.daemon.protocol import DaemonRequest
from agent_sec_cli.daemon.registry import (
    HandlerResult,
    MethodRegistry,
    MethodSpec,
)
from agent_sec_cli.daemon.runtime import (
    DaemonRuntime,
    PromptScanRuntimeState,
)
from agent_sec_cli.security_middleware.result import ActionResult

_MODEL_FREE_MODES = frozenset({"fast"})
# Modes that depend on the BERT-based L2/L3 model and are degraded to FAST
# when that model is not yet loaded.
_MODEL_REQUIRED_MODES = frozenset({"standard", "strict"})
# The complete set of modes the daemon is willing to route. Anything outside
# this set — including the beta ``multi_turn`` L4 mode, which calls Ollama
# directly and is never routed through the daemon — is rejected up front so
# callers get a stable, daemon-owned error rather than either a
# mis-parameterised backend call or a backend message that advertises modes
# the daemon itself refuses.
_SUPPORTED_MODES = _MODEL_FREE_MODES | _MODEL_REQUIRED_MODES


def register_prompt_scan_methods(registry: MethodRegistry) -> None:
    """Register prompt scanner daemon methods."""
    registry.register(
        MethodSpec(
            method="scan-prompt",
            handler=prompt_scan_handler,
            lifecycle="security action",
            queue="prompt-scan",
            timeout_ms=30_000,
            access_log=True,
        )
    )


async def prompt_scan_handler(
    request: DaemonRequest, runtime: DaemonRuntime
) -> HandlerResult:
    """Execute prompt scanning through security middleware.

    When the ML model is not yet loaded (cold-start window), the handler
    degrades gracefully to FAST mode (L1 rule-engine only) instead of
    returning verdict=error.  The response carries ``degraded=true`` and a
    ``degraded_reason`` field so callers can distinguish full-fidelity
    results from rule-only fallback results.

    Verdict semantics change under degradation: in STANDARD/STRICT mode an
    L1 rule-engine hit that the L2 ML classifier would *not* confirm yields
    ``WARN`` (a possible false-positive), but in degraded FAST mode L1 is
    the sole authority so the same input yields ``DENY``.  Callers that want
    to avoid false-positive blocks during the cold-start window may treat a
    degraded ``DENY`` as a ``WARN`` (alert and log without blocking).

    Modes outside ``fast``/``standard``/``strict`` (notably the beta
    ``multi_turn`` L4 mode) are rejected up front with ``BadRequestError``.
    """
    params = request.params
    requested_mode = _string_param(params, "mode", default="standard").lower()

    if requested_mode not in _SUPPORTED_MODES:
        allowed = ", ".join(sorted(_SUPPORTED_MODES))
        raise BadRequestError(
            f"prompt_scan mode '{requested_mode}' must be one of: {allowed}"
        )

    text = _string_param(params, "text")
    source = _string_param(params, "source")

    prompt_scan_state = runtime.prompt_scan_state
    model_ready = prompt_scan_state.status == "ready" and prompt_scan_state.loaded

    # Determine effective mode and whether we are degrading.
    degraded = False
    degraded_reason = ""
    if requested_mode in _MODEL_FREE_MODES or model_ready:
        # Fast mode never needs the model; other modes pass through when ready.
        effective_mode = requested_mode
    else:
        # Model-dependent mode (standard/strict) but model not ready — fall
        # back to FAST (L1 rule engine only).
        effective_mode = "fast"
        degraded = True
        degraded_reason = _build_degraded_reason(prompt_scan_state, requested_mode)

    result = await asyncio.to_thread(
        _invoke_prompt_scan,
        text=text,
        mode=effective_mode,
        source=source,
    )

    if degraded:
        result = _inject_degraded_metadata(result, degraded_reason)

    return _action_result_to_handler_result(result)


def _invoke_prompt_scan(
    *,
    text: str,
    mode: str,
    source: str,
) -> Any:
    from agent_sec_cli.security_middleware import (  # noqa: PLC0415 - lazy import: daemon handler execution only
        invoke,
    )

    return invoke(
        "prompt_scan",
        caller="daemon",
        text=text,
        mode=mode,
        source=source,
    )


def _build_degraded_reason(state: PromptScanRuntimeState, requested_mode: str) -> str:
    """Build the ``degraded_reason`` string for a FAST fallback.

    A status of ``degraded`` is permanent — the one-shot preload job failed
    and will not retry until the daemon restarts, so operators need an
    explicit warmup hint plus the underlying ``last_error`` to distinguish
    it from a transient cold-start (pending/downloading/loading).
    """
    parts = [
        f"model not ready (status={state.status})",
        (
            f"degraded from mode='{requested_mode}' to mode='fast' "
            "(L1 rule-engine only)"
        ),
    ]
    if state.status == "degraded":
        parts.append(
            "preload failed, run `agent-sec-cli scan-prompt warmup` "
            "then restart the daemon"
        )
    if state.model:
        parts.append(f"model={state.model}")
    if state.last_error:
        parts.append(f"last_error={state.last_error}")
    return ", ".join(parts)


def _inject_degraded_metadata(result: ActionResult, reason: str) -> ActionResult:
    """Inject degraded=true into an ActionResult's data dict (non-mutating)."""
    data = dict(result.data) if result.data else {}
    data["degraded"] = True
    data["degraded_reason"] = reason
    stdout = json.dumps(data, indent=2, ensure_ascii=False)
    return ActionResult(
        success=result.success,
        data=data,
        stdout=stdout,
        error=result.error,
        exit_code=result.exit_code,
    )


def _action_result_to_handler_result(result: Any) -> HandlerResult:
    return HandlerResult(
        data=result.data,
        stdout=result.stdout,
        stderr=result.error,
        exit_code=result.exit_code,
    )


def _string_param(
    params: dict[str, Any],
    name: str,
    default: str = "",
) -> str:
    value = params.get(name, default)
    if value is None:
        return default
    return str(value)
