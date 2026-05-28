"""ws-ckpt Hermes plugin — workspace checkpoint on each conversation turn.

Implements the Hermes Plugin interface: ``register(ctx)`` is called once at
plugin load time and registers three hooks:

- ``on_session_start`` — create an initial baseline checkpoint.
- ``pre_llm_call``     — capture the latest user message for later use.
- ``on_session_end``   — create a turn-end checkpoint with the captured message.

Note: Hermes fires ``on_session_end`` at the end of every ``run_conversation()``
call, which is per-turn (one user message), not per-session.
"""

from __future__ import annotations

import os
import secrets
import threading
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Optional

from .checkpoint_manager import CheckpointManager
from .config import MSG_TRUNCATE_LEN, load_config
from .tools import TOOLS, check_ws_ckpt_available

# ---------------------------------------------------------------------------
# Module-level state
# ---------------------------------------------------------------------------

_manager: Optional[CheckpointManager] = None
_last_user_message: str = ""
_msg_lock = threading.Lock()


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _get_manager() -> CheckpointManager:
    """Return (or create) the singleton CheckpointManager."""
    global _manager
    if _manager is None:
        config = load_config()
        _manager = CheckpointManager(config)
        print("[ws-ckpt] Plugin initialized", flush=True)
    return _manager


# init/checkpoint/rollback all swap the workspace inode (remove_dir_all → btrfs
# subvolume symlink, then later snapshot/rollback recreates it), so any process
# holding cwd inside the workspace will get ENOENT on the next getcwd(). Refuse
# instead of silently producing broken state.
CWD_INSIDE_WORKSPACE_REASON = (
    "The hosting process's cwd is inside the workspace. "
    "ws-ckpt replaces the workspace inode during init/checkpoint/rollback, "
    "which would invalidate the process cwd. "
    "The user must launch the session from outside the workspace directory."
)


def _cwd_inside_workspace(workspace: str) -> bool:
    """Return True when the current cwd is the workspace itself or a descendant."""
    try:
        cwd = Path(os.getcwd()).resolve()
    except (FileNotFoundError, OSError):
        # cwd already invalid — caller decides what to do; we can't prove containment.
        return False
    try:
        ws_path = Path(workspace).resolve()
    except (FileNotFoundError, OSError):
        return False
    return cwd == ws_path or ws_path in cwd.parents


# ---------------------------------------------------------------------------
# Hook callbacks
# ---------------------------------------------------------------------------


def _on_session_start(session_id: str = "", model: str = "", **_: Any) -> None:
    """Handle on_session_start — init the workspace then create a baseline checkpoint."""
    manager = _get_manager()

    if not manager.config.auto_checkpoint:
        return

    if not manager.config.workspace:
        manager.set_auto_checkpoint(False)
        print(
            "[ws-ckpt] No workspace configured — auto-checkpoint disabled",
            flush=True,
        )
        return

    if _cwd_inside_workspace(manager.config.workspace):
        # Disable for this session so on_session_end stops trying too.
        manager.set_auto_checkpoint(False)
        print(
            f"[ws-ckpt] Refusing auto-checkpoint: {CWD_INSIDE_WORKSPACE_REASON}",
            flush=True,
        )
        return

    # Idempotent: ws-ckpt init is a no-op if the workspace is already registered,
    # so eager-init here avoids the implicit init-on-first-checkpoint cost.
    init_output = manager.init_workspace()
    if init_output.exit_code != 0:
        print(
            f"[ws-ckpt] init failed ✗ {init_output.stderr.strip() or init_output.stdout.strip()}",
            flush=True,
        )
        return

    snapshot_id = secrets.token_hex(4)
    timestamp = datetime.now(timezone.utc).isoformat()

    metadata = {
        "event": "session_start",
        "turn": 0,
        "timestamp": timestamp,
    }

    result = manager.create_checkpoint(
        snapshot_id=snapshot_id,
        message="session-start",
        metadata=metadata,
    )

    if result.success:
        print(f"[ws-ckpt] Initial snapshot saved ✓ {result.snapshot}", flush=True)
    else:
        print(f"[ws-ckpt] Initial snapshot failed ✗ {result.message}", flush=True)


def _on_pre_llm_call(
    user_message: str = "",
    session_id: str = "",
    is_first_turn: bool = False,
    **_: Any,
) -> None:
    """Capture the latest user message for use in on_session_end."""
    global _last_user_message
    with _msg_lock:
        _last_user_message = user_message


def _on_session_end(
    session_id: str = "",
    completed: bool = True,
    interrupted: bool = False,
    **_: Any,
) -> None:
    """Handle on_session_end — create a checkpoint after the turn."""
    manager = _get_manager()

    if not manager.config.auto_checkpoint:
        return

    # Retrieve the user message captured by pre_llm_call
    with _msg_lock:
        raw_message = _last_user_message

    if isinstance(raw_message, str) and raw_message:
        truncated_message = raw_message[:MSG_TRUNCATE_LEN]
        if len(raw_message) > MSG_TRUNCATE_LEN:
            truncated_message += "..."
    else:
        truncated_message = "agent turn"

    turn = manager.advance_turn()
    snapshot_id = secrets.token_hex(4)
    timestamp = datetime.now(timezone.utc).isoformat()

    metadata = {
        "event": "turn_end",
        "turn": turn,
        "timestamp": timestamp,
        "success": completed,
    }

    result = manager.create_checkpoint(
        snapshot_id=snapshot_id,
        message=truncated_message,
        metadata=metadata,
    )

    if result.success:
        print(f"[ws-ckpt] Turn {turn} snapshot saved ✓ {result.snapshot}", flush=True)
    else:
        print(f"[ws-ckpt] Turn {turn} snapshot failed ✗ {result.message}", flush=True)


# ---------------------------------------------------------------------------
# Plugin registration entry point
# ---------------------------------------------------------------------------


def register(ctx) -> None:  # noqa: ANN001
    """Register ws-ckpt hooks and tools with the Hermes plugin system."""
    ctx.register_hook("on_session_start", _on_session_start)
    ctx.register_hook("pre_llm_call", _on_pre_llm_call)
    ctx.register_hook("on_session_end", _on_session_end)

    # Register tools
    for name, schema, handler, emoji in TOOLS:
        ctx.register_tool(
            name=name,
            toolset="ws-ckpt",
            schema=schema,
            handler=handler,
            check_fn=check_ws_ckpt_available,
            emoji=emoji,
        )
