"""Daemon handler for the scan-prompt CLI-compatible method."""

import asyncio
import copy
import json
from typing import Any

from agent_sec_cli.daemon.errors import UnavailableError
from agent_sec_cli.daemon.protocol import DaemonRequest
from agent_sec_cli.daemon.registry import (
    HandlerResult,
    MethodRegistry,
    MethodSpec,
)
from agent_sec_cli.daemon.runtime import DaemonRuntime


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
    """Execute prompt scanning through security middleware."""
    prompt_scan_state = runtime.prompt_scan_state
    if prompt_scan_state.status != "ready" or not prompt_scan_state.loaded:
        # Cold-start degradation: when the ML model (L2) is not ready,
        # degrade to fast mode (L1 rule-engine only) instead of returning
        # an error.  This keeps the security scanner online during the
        # model download/load window.  L1 DENY is rewritten to WARN to
        # avoid false blocks during the degraded window.
        mode = _string_param(request.params, "mode", default="standard")
        if mode in ("standard", "strict"):
            return await _degraded_scan(request, runtime, mode)
        raise UnavailableError(_prompt_unavailable_message(runtime))

    params = request.params
    result = await asyncio.to_thread(
        _invoke_prompt_scan,
        text=_string_param(params, "text"),
        mode=_string_param(params, "mode", default="standard"),
        source=_string_param(params, "source"),
    )
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


async def _degraded_scan(
    request: DaemonRequest,
    runtime: DaemonRuntime,
    original_mode: str,
) -> HandlerResult:
    """Run a fast-mode (L1-only) scan and tag the result as degraded.

    When the ML model is not ready, fall back to L1 rule-engine scanning
    so the scanner stays functional during cold start.  The response carries
    ``degraded=true``, ``degraded_reason``, and ``degraded_original_verdict``
    for audit purposes.  Any L1 DENY is rewritten to WARN to avoid false
    blocks during the degraded window.
    """
    result = await asyncio.to_thread(
        _invoke_prompt_scan,
        text=_string_param(request.params, "text"),
        mode="fast",
        source=_string_param(request.params, "source"),
    )
    data = copy.deepcopy(result.data) if result.data else {}
    original_verdict = data.get("verdict", "pass")
    # Rewrite DENY → WARN during degraded mode to avoid cold-start false blocks.
    if data.get("verdict") == "deny":
        data["verdict"] = "warn"
        data["ok"] = True
        data["risk_level"] = "medium"
    data["degraded"] = True
    data["degraded_reason"] = (
        f"model not ready (status={runtime.prompt_scan_state.status}), "
        f"degraded from {original_mode} to fast (L1 rule-engine only)"
    )
    data["degraded_original_verdict"] = original_verdict
    stdout = json.dumps(data, indent=2, ensure_ascii=False)
    return HandlerResult(data=data, stdout=stdout, stderr="", exit_code=0)


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


def _prompt_unavailable_message(runtime: DaemonRuntime) -> str:
    prompt_scan_state = runtime.prompt_scan_state.to_dict()
    status = prompt_scan_state.get("status", "unknown")
    model = prompt_scan_state.get("model")
    last_error = prompt_scan_state.get("last_error")

    if status == "downloading":
        parts = [
            "prompt scanner is not ready: model download is still in progress",
            "status=downloading",
        ]
    elif status == "loading":
        parts = [
            "prompt scanner is not ready: model download completed and the model is loading",
            "status=loading",
        ]
    elif status == "degraded":
        parts = [
            "prompt scanner preload failed",
            "retry with `agent-sec-cli scan-prompt warmup`",
            "then restart the agent-sec daemon process",
            "status=degraded",
        ]
    else:
        parts = [f"prompt scanner is not ready: status={status}"]

    if model:
        parts.append(f"model={model}")
    if last_error:
        parts.append(f"last_error={last_error}")
    return ", ".join(parts)
