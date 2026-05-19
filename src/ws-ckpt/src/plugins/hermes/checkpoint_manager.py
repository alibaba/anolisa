"""CLI wrapper and snapshot management for the ws-ckpt Hermes plugin.

Wraps all `ws-ckpt` CLI invocations using subprocess.run.
Each method constructs the appropriate CLI arguments, executes the command,
and returns structured results.
"""

from __future__ import annotations

import json
import subprocess
from dataclasses import dataclass
from typing import Any, Dict, List, Optional

from .config import HermesPluginConfig, MSG_TRUNCATE_LEN

DEFAULT_TIMEOUT_S = 30

WS_CKPT_BIN = "ws-ckpt"


@dataclass
class CommandOutput:
    """Structured output from a CLI invocation."""

    exit_code: int
    stdout: str
    stderr: str


@dataclass
class CheckpointResult:
    """Result of a checkpoint creation attempt."""

    success: bool
    message: str
    snapshot: str = ""
    skipped: bool = False
    reason: Optional[str] = None


def map_error_to_message(stderr: str, context: Optional[Dict[str, Any]] = None) -> str:
    """Map CLI stderr to a user-friendly error message.

    Follows the OpenClaw mapErrorToLLMMessage pattern.
    """
    ctx_str = ""
    if context:
        ctx_str = f" (context: {json.dumps(context)})"

    lowered = stderr.lower()

    if "not initialized" in lowered:
        return f"Workspace not initialized for ws-ckpt.{ctx_str}"
    # CLI environment issues take priority over generic "not found" (snapshot)
    if "binary not found" in lowered or "not found on path" in lowered:
        return f"ws-ckpt CLI not found on PATH.{ctx_str}"
    if "not found" in lowered or "no such" in lowered:
        return f"Snapshot not found.{ctx_str}"
    if "daemon" in lowered or "connection" in lowered:
        return f"ws-ckpt daemon is not responding. Is it running?{ctx_str}"
    if "permission" in lowered:
        return f"Permission denied.{ctx_str}"
    if "timeout" in lowered:
        return f"Command timed out.{ctx_str}"

    return f"ws-ckpt error: {stderr.strip()}{ctx_str}"


class CheckpointManager:
    """Manages ws-ckpt CLI operations.

    Provides synchronous methods for initializing the workspace and creating
    checkpoints. The plugin does not maintain an in-memory snapshot cache —
    `ws-ckpt list` is the single source of truth, queried on demand by tools.
    """

    def __init__(self, config: HermesPluginConfig) -> None:
        self._config = config
        self._turn_count: int = 0

    @property
    def config(self) -> HermesPluginConfig:
        """Expose the plugin config for hooks and tool handlers."""
        return self._config

    def set_workspace(self, workspace: str) -> None:
        """Update the in-process workspace path."""
        self._config.workspace = workspace

    def set_auto_checkpoint(self, enabled: bool) -> None:
        """Update the in-process auto-checkpoint flag."""
        self._config.auto_checkpoint = enabled

    def advance_turn(self) -> int:
        """Increment and return the turn counter."""
        self._turn_count += 1
        return self._turn_count

    # ------------------------------------------------------------------
    # CLI execution
    # ------------------------------------------------------------------

    def _run(self, args: List[str]) -> CommandOutput:
        """Execute a ws-ckpt CLI command and return structured output."""
        try:
            result = subprocess.run(
                [WS_CKPT_BIN, *args],
                capture_output=True,
                text=True,
                timeout=DEFAULT_TIMEOUT_S,
            )
            return CommandOutput(
                exit_code=result.returncode,
                stdout=result.stdout,
                stderr=result.stderr,
            )
        except subprocess.TimeoutExpired:
            return CommandOutput(
                exit_code=1,
                stdout="",
                stderr=f"Command timed out after {DEFAULT_TIMEOUT_S} seconds",
            )
        except FileNotFoundError:
            return CommandOutput(
                exit_code=127,
                stdout="",
                stderr=f"{WS_CKPT_BIN} binary not found on PATH",
            )
        except Exception as e:
            return CommandOutput(
                exit_code=1,
                stdout="",
                stderr=str(e),
            )

    # ------------------------------------------------------------------
    # High-level operations
    # ------------------------------------------------------------------

    def init_workspace(self) -> CommandOutput:
        """Initialize a workspace for ws-ckpt management.

        Equivalent to: ws-ckpt init --workspace <ws>
        """
        return self._run(["init", "--workspace", self._config.workspace])

    def create_checkpoint(
        self,
        snapshot_id: str,
        message: str = "",
        metadata: Optional[Dict[str, Any]] = None,
    ) -> CheckpointResult:
        """Create a checkpoint (snapshot) of the workspace.

        Equivalent to:
            ws-ckpt checkpoint --workspace <ws> --id <id> [--message <msg>] [--metadata <json>]
        """
        args = [
            "checkpoint",
            "--workspace", self._config.workspace,
            "--id", snapshot_id,
        ]

        if message:
            args.extend(["--message", message[:MSG_TRUNCATE_LEN]])

        if metadata:
            args.extend(["--metadata", json.dumps(metadata)])

        output = self._run(args)

        if output.exit_code != 0:
            return CheckpointResult(
                success=False,
                message=map_error_to_message(output.stderr, {"id": snapshot_id}),
            )

        return CheckpointResult(
            success=True,
            message=f"Checkpoint created: {snapshot_id}",
            snapshot=snapshot_id,
        )
