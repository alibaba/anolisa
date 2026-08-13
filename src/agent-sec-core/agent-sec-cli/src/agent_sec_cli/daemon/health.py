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
        # Compatibility stub: scanning moved in-process to the Rust extension,
        # so the daemon no longer tracks scanner state. Keep the key with the
        # legacy PromptScanRuntimeState shape frozen at its success terminal
        # state (ready/loaded) so monitors reading `.prompt_scan.*` neither
        # fail silently nor raise false not-ready alerts; "native" in the
        # model field marks the in-process engine.
        "prompt_scan": {
            "status": "ready",
            "model": "native",
            "loaded": True,
            "last_error": None,
            "last_started_at": None,
            "last_finished_at": None,
        },
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
