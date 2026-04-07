"""{backend_name} backend — delegates compute to Rust extension."""

from __future__ import annotations

import json
import os
import sys

from agent_sec_cli.security_middleware.result import ActionResult

# Ensure rust_ext/ directory is on sys.path for the Rust .so
_RUST_EXT = os.path.join(os.path.dirname(__file__), "..", "..", "rust_ext")
if os.path.isdir(_RUST_EXT):
    sys.path.insert(0, _RUST_EXT)

try:
    import rust_backends
    RUST_AVAILABLE = True
except ImportError:
    RUST_AVAILABLE = False


class {BackendName}Backend:
    """Backend for {backend_name} — uses Rust when available, Python fallback."""

    def execute(self, ctx, **kwargs) -> ActionResult:
        """Execute the backend logic.

        Args:
            ctx: Request context (unused beyond tracing).
            **kwargs: Backend-specific parameters passed from CLI.

        Returns:
            ActionResult with success status, output, and exit code.
        """
        if RUST_AVAILABLE:
            return self._execute_rust(**kwargs)
        return self._execute_python(**kwargs)

    def _execute_rust(self, **kwargs) -> ActionResult:
        """Execute using Rust extension."""
        try:
            req = json.dumps(kwargs)
            resp_json = rust_backends.{action_name}(req)
            resp = json.loads(resp_json)
            return ActionResult(
                success=True,
                data=resp,
                stdout=self._format_stdout(resp),
            )
        except Exception as exc:
            return ActionResult(success=False, error=f"Rust error: {exc}", exit_code=1)

    def _execute_python(self, **kwargs) -> ActionResult:
        """Pure Python fallback — implement a minimal version here."""
        data = {**kwargs, "note": "python fallback — Rust extension not available"}
        return ActionResult(
            success=True,
            data=data,
            stdout=json.dumps(data, indent=2),
        )

    @staticmethod
    def _format_stdout(resp: dict) -> str:
        """Build human-readable output from the Rust response.

        Customize this method to format the Rust response for terminal output.
        Must return a non-empty string so the CLI has something to print.
        """
        # TODO: Implement domain-specific formatting
        return json.dumps(resp, indent=2)
