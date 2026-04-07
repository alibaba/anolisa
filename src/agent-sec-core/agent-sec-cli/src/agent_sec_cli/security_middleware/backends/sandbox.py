"""Sandbox prehook backend — log sandbox decisions, future: evaluate isolation correctness."""

from __future__ import annotations

from agent_sec_cli.security_middleware.result import ActionResult


class SandboxBackend:
    """Record sandbox decisions for auditing.

    Currently logging-only (the lifecycle layer handles event emission).
    Future: evaluate isolation policy correctness here.
    """

    def execute(
        self,
        ctx,
        decision: str = "",
        command: str = "",
        reasons: str = "",
        network_policy: str = "",
        cwd: str = "",
        **kwargs,
    ) -> ActionResult:
        """Record sandbox decision.  Currently logging-only (via lifecycle).

        Future: evaluate isolation policy correctness here.
        """
        return ActionResult(
            success=True,
            data={
                "decision": decision,
                "command": command,
                "reasons": reasons,
                "network_policy": network_policy,
                "cwd": cwd,
            },
        )
