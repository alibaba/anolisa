"""Daemon handler for the scan-prompt CLI-compatible method."""

import asyncio
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
    if runtime.prompt_scan.status != "ready" or not runtime.prompt_scan.loaded:
        raise UnavailableError(_prompt_unavailable_message(runtime))

    params = request.params
    result = await asyncio.to_thread(
        _invoke_prompt_scan,
        trace_context=request.trace_context,
        text=_string_param(params, "text"),
        mode=_string_param(params, "mode", default="standard"),
        source=_string_param(params, "source"),
    )
    return _action_result_to_handler_result(result)


def _invoke_prompt_scan(
    *,
    trace_context: dict[str, Any],
    text: str,
    mode: str,
    source: str,
) -> Any:
    from agent_sec_cli.security_middleware import (  # noqa: PLC0415 - lazy import: daemon handler execution only
        invoke_with_context,
    )

    return invoke_with_context(
        "prompt_scan",
        caller="daemon",
        trace_context=trace_context,
        text=text,
        mode=mode,
        source=source,
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


def _prompt_unavailable_message(runtime: DaemonRuntime) -> str:
    prompt_state = runtime.prompt_scan.to_dict()
    status = prompt_state.get("status", "unknown")
    model = prompt_state.get("model")
    last_error = prompt_state.get("last_error")

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
