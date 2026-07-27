"""Daemon health method."""

import os
from typing import Any

from agent_sec_cli.daemon.protocol import DaemonRequest
from agent_sec_cli.daemon.registry import (
    HandlerResult,
    MethodRegistry,
    MethodSpec,
)
from agent_sec_cli.daemon.runtime import DaemonRuntime


def build_health_snapshot(runtime: DaemonRuntime) -> dict[str, Any]:
    """Build the daemon.health response without initializing heavy modules."""
    return {
        "status": runtime.status,
        "pid": os.getpid(),
        "uptime_seconds": runtime.uptime_seconds(),
        "socket": str(runtime.socket_path),
        "prompt_scan": runtime.prompt_scan_state.to_dict(),
        "jobs": runtime.jobs.status(),
        "queues": runtime.queues.to_dict(),
    }


def health_handler(_request: DaemonRequest, runtime: DaemonRuntime) -> HandlerResult:
    """Return daemon runtime health."""
    return HandlerResult(data=build_health_snapshot(runtime))


def register_health_methods(registry: MethodRegistry) -> None:
    """Register daemon health methods."""
    registry.register(
        MethodSpec(
            method="daemon.health",
            handler=health_handler,
            lifecycle="admin",
            queue="admin",
            timeout_ms=1000,
            access_log=False,
        )
    )
