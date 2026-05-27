"""Agent-facing tools for the ws-ckpt Hermes plugin.

Tool surface mirrors the OpenClaw plugin (`ws-ckpt-*`):

  ws-ckpt-config     — view or update plugin/daemon configuration
  ws-ckpt-checkpoint — create a new snapshot
  ws-ckpt-rollback   — rollback to a specific snapshot
  ws-ckpt-list       — list snapshots for the workspace
  ws-ckpt-diff       — show file changes between two snapshots
  ws-ckpt-delete     — delete a snapshot
  ws-ckpt-status     — show workspace checkpoint status
"""

from __future__ import annotations

import json
import shutil
import subprocess
from typing import Any, Dict, Optional, Tuple

from .config import load_config


# Cached once per process: ws-ckpt is a system-installed binary, so a path
# lookup at first call survives the rest of the session.
_ws_ckpt_available: Optional[bool] = None


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _get_default_workspace() -> str:
    """Resolve workspace via the singleton manager's config.

    Reads from the same in-memory state hooks use, so an
    `ws-ckpt-config update workspace` takes effect immediately and isn't
    shadowed by a stale env var on the next tool call.
    """
    from . import _get_manager  # lazy: __init__ imports tools

    return _get_manager().config.workspace


_NO_WORKSPACE_MSG = "No workspace configured. Tell me the workspace path and I'll set it up."


def _require_workspace() -> Tuple[str, Optional[str]]:
    """Resolve and validate workspace. Returns (workspace, None) or ("", error_json)."""
    ws = _get_default_workspace()
    if not ws:
        return "", _err(_NO_WORKSPACE_MSG)
    return ws, None


def _reject_if_cwd_inside_workspace(workspace: str) -> Optional[str]:
    """Return a serialized error response when cwd is inside workspace, else None."""
    from . import CWD_INSIDE_WORKSPACE_REASON, _cwd_inside_workspace  # lazy

    if _cwd_inside_workspace(workspace):
        return _err(f"Refusing: {CWD_INSIDE_WORKSPACE_REASON}")
    return None


def _run_ws_ckpt_cmd(cmd: list) -> Tuple[bool, str]:
    """Execute a ws-ckpt CLI command and return (success, output)."""
    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
        return result.returncode == 0, result.stdout.strip() or result.stderr.strip()
    except subprocess.TimeoutExpired:
        return False, "Command timed out (30s)"
    except FileNotFoundError:
        return False, "ws-ckpt not found. Is it installed and in PATH?"
    except Exception as e:
        return False, str(e)


def _json(obj: Any) -> str:
    return json.dumps(obj, ensure_ascii=False)


def _ok(output: str) -> str:
    return _json({"success": True, "output": output})


def _err(msg: str) -> str:
    return _json({"success": False, "error": msg})


# ---------------------------------------------------------------------------
# Runtime gate
# ---------------------------------------------------------------------------


def check_ws_ckpt_available() -> bool:
    """Return True when ws-ckpt CLI is on PATH.

    Hermes' registry caches check_fn results for 30s, but we cache for the
    full process lifetime: ws-ckpt is a system-installed binary and a PATH
    lookup is enough — no need to fork `ws-ckpt --version` on every gate.
    """
    global _ws_ckpt_available
    if _ws_ckpt_available is None:
        _ws_ckpt_available = shutil.which("ws-ckpt") is not None
    return _ws_ckpt_available


# ---------------------------------------------------------------------------
# Schemas (OpenAI Function Calling format)
# ---------------------------------------------------------------------------

WS_CKPT_CONFIG_SCHEMA: Dict[str, Any] = {
    "name": "ws-ckpt-config",
    "description": (
        "View or update ws-ckpt configuration. "
        "Configurable keys: "
        "autoCheckpoint (whether to auto-snapshot at the end of each conversation turn), "
        "workspace (default workspace absolute path; used by every command without -w. "
        "If the path is a symlink, use the link itself — do NOT replace it with the "
        "resolved real path; the daemon registers and matches by the exact string you pass), "
        "maxSnapshotsNum (number of snapshots to keep when auto-cleanup is by count), "
        "maxSnapshotsDuration (duration to keep when auto-cleanup is by time, e.g. \"7d\"/\"24h\"). "
        "Only update the specific key requested by the user."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "description": 'Action to perform: "view" (default) or "update"',
            },
            "key": {
                "type": "string",
                "description": (
                    "Config key to update: autoCheckpoint, workspace, "
                    "maxSnapshotsNum, maxSnapshotsDuration"
                ),
            },
            "value": {
                "type": "string",
                "description": (
                    "New value as a string. Formats: "
                    "autoCheckpoint = \"true\"/\"false\"; "
                    "workspace = absolute path; "
                    "maxSnapshotsNum = positive integer or \"unset\"; "
                    "maxSnapshotsDuration = e.g. \"7d\"/\"24h\" or \"unset\"."
                ),
            },
        },
        "additionalProperties": False,
    },
}

WS_CKPT_CHECKPOINT_SCHEMA: Dict[str, Any] = {
    "name": "ws-ckpt-checkpoint",
    "description": (
        "Create a checkpoint of the default or specified workspace. Use this "
        "to save the current state before making significant changes, so you "
        "can rollback if needed."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "Required: caller-provided snapshot identifier",
            },
            "message": {
                "type": "string",
                "description": "Optional message describing the checkpoint",
            },
            "workspace": {
                "type": "string",
                "description": (
                    "Optional: workspace absolute path. Defaults to the "
                    "configured workspace. If the path is a symlink, use the "
                    "link itself — do NOT replace it with the resolved real path."
                ),
            },
        },
        "required": ["id"],
        "additionalProperties": False,
    },
}

WS_CKPT_ROLLBACK_SCHEMA: Dict[str, Any] = {
    "name": "ws-ckpt-rollback",
    "description": (
        "Roll back the workspace to a specific checkpoint. Use ws-ckpt-list "
        "first to see available snapshots."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "target": {
                "type": "string",
                "description": "Snapshot id to roll back to.",
            },
            "workspace": {
                "type": "string",
                "description": (
                    "Optional: workspace absolute path. Defaults to the "
                    "configured workspace. If the path is a symlink, use the "
                    "link itself — do NOT replace it with the resolved real path."
                ),
            },
        },
        "required": ["target"],
        "additionalProperties": False,
    },
}

WS_CKPT_LIST_SCHEMA: Dict[str, Any] = {
    "name": "ws-ckpt-list",
    "description": (
        "List all snapshots managed by ws-ckpt. Always display the FULL "
        "untruncated result to the user."
    ),
    "parameters": {
        "type": "object",
        "properties": {},
        "additionalProperties": False,
    },
}

WS_CKPT_DIFF_SCHEMA: Dict[str, Any] = {
    "name": "ws-ckpt-diff",
    "description": (
        "Compare file changes between two snapshots. Always display the "
        "FULL untruncated result to the user. Do NOT re-interpret or "
        "contradict the tool output."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "from": {
                "type": "string",
                "description": "Source snapshot id",
            },
            "to": {
                "type": "string",
                "description": (
                    "Target snapshot id or name (defaults to current state)"
                ),
            },
        },
        "required": ["from", "to"],
        "additionalProperties": False,
    },
}

WS_CKPT_DELETE_SCHEMA: Dict[str, Any] = {
    "name": "ws-ckpt-delete",
    "description": (
        "Delete a specific snapshot. Use ws-ckpt-list to see available "
        "snapshots before deleting."
    ),
    "parameters": {
        "type": "object",
        "properties": {
            "snapshot": {
                "type": "string",
                "description": "Required: snapshot ID to delete",
            },
            "workspace": {
                "type": "string",
                "description": (
                    "Optional: workspace absolute path. Defaults to the "
                    "configured workspace. If the path is a symlink, use the "
                    "link itself — do NOT replace it with the resolved real path."
                ),
            },
        },
        "required": ["snapshot"],
        "additionalProperties": False,
    },
}

WS_CKPT_STATUS_SCHEMA: Dict[str, Any] = {
    "name": "ws-ckpt-status",
    "description": (
        "Show ws-ckpt service status and workspace information. Returns the "
        "complete status from ws-ckpt daemon — no additional CLI or exec "
        "verification needed."
    ),
    "parameters": {
        "type": "object",
        "properties": {},
        "additionalProperties": False,
    },
}


# ---------------------------------------------------------------------------
# Handlers
# ---------------------------------------------------------------------------


def handle_ws_ckpt_config(args: Dict[str, Any], **_kwargs) -> str:
    """Handle ws-ckpt-config tool call.

    view  → print plugin config + transparently dump `ws-ckpt config` stdout.
    update autoCheckpoint / workspace  → mutate the in-process manager
        config; persistence requires editing ~/.hermes/config.yaml.
    update maxSnapshotsNum / maxSnapshotsDuration → shell out to
        `ws-ckpt config --enable-auto-cleanup --auto-cleanup-keep <v>`.
    """
    action = (args.get("action") or "view").strip().lower()

    if action == "view":
        cfg = load_config()
        lines = [
            "Current ws-ckpt plugin configuration:",
            f"  autoCheckpoint: {cfg.auto_checkpoint}",
            f"  workspace:      {cfg.workspace}",
            "",
            "Daemon configuration (from `ws-ckpt config`):",
        ]
        success, output = _run_ws_ckpt_cmd(["ws-ckpt", "config"])
        lines.append(output if output else "(daemon returned no output)")
        if not success:
            lines.append("(failed to query daemon — output above is stderr)")
        return _ok("\n".join(lines))

    if action not in ("update", "set"):
        return _err(f'Unknown action: {action}. Use "view" or "update".')

    key = (args.get("key") or "").strip()
    value = args.get("value")
    if not key:
        return _err(
            "Usage: ws-ckpt-config update <key> <value>. "
            "Available keys: autoCheckpoint, workspace, "
            "maxSnapshotsNum, maxSnapshotsDuration."
        )

    # Daemon-level keys: persist via `ws-ckpt config`
    if key in ("maxSnapshotsNum", "maxSnapshotsDuration"):
        if value is None:
            return _err(
                f"{key} requires a value (or \"unset\" to disable auto-cleanup)"
            )
        value = str(value).strip()

        if value == "unset":
            success, output = _run_ws_ckpt_cmd(
                ["ws-ckpt", "config", "--disable-auto-cleanup"]
            )
            if not success:
                return _err(f"Failed to disable auto-cleanup: {output}")
            return _ok(f"Cleared: {key} unset — auto-cleanup disabled.")

        if key == "maxSnapshotsNum":
            try:
                num = int(value)
                if num < 1:
                    raise ValueError
            except ValueError:
                return _err("maxSnapshotsNum must be a positive integer")
            keep = str(num)
        else:
            keep = value  # daemon parses duration strings like "7d", "24h"

        success, output = _run_ws_ckpt_cmd(
            ["ws-ckpt", "config", "--enable-auto-cleanup",
             "--auto-cleanup-keep", keep]
        )
        if not success:
            return _err(f"Failed to configure daemon: {output}")
        return _ok(
            f"Updated daemon config: {key} = {keep} "
            f"(auto-cleanup enabled, keep {keep})"
        )

    # Plugin-level keys: persist to ~/.hermes/config.yaml AND sync the
    # singleton manager's config in-place so the change takes effect this
    # session without re-reading yaml on every hook fire.
    if key == "autoCheckpoint":
        if value is None:
            return _err("autoCheckpoint requires a value (true/false)")
        coerced = str(value).strip().lower() in ("true", "1", "yes", "on")
        if coerced:
            workspace, ws_err = _require_workspace()
            if ws_err:
                return ws_err
            rejection = _reject_if_cwd_inside_workspace(workspace)
            if rejection:
                return rejection
        err = _persist_plugin_yaml(autoCheckpoint=coerced)
        if err:
            return _err(f"Failed to persist config: {err}")
        from . import _get_manager  # local: __init__ imports tools
        _get_manager().set_auto_checkpoint(coerced)
        return _ok(f"Config updated: autoCheckpoint = {coerced}")

    if key == "workspace":
        if not value:
            return _err("workspace requires a path value")
        new_path = str(value).strip()
        err = _persist_plugin_yaml(workspace=new_path)
        if err:
            return _err(f"Failed to persist config: {err}")
        from . import _get_manager  # local: __init__ imports tools
        _get_manager().set_workspace(new_path)
        return _ok(f"Config updated: workspace = {new_path}")

    return _err(
        f"Unknown config key: {key}. Available: autoCheckpoint, "
        "workspace, maxSnapshotsNum, maxSnapshotsDuration."
    )


def _persist_plugin_yaml(**fields: Any) -> str:
    """Write ``plugins.ws-ckpt.<key> = value`` into ~/.hermes/config.yaml.

    Returns an error message on failure, empty string on success.
    Refuses to write when the Hermes installation is managed.
    """
    try:
        from hermes_cli.config import (
            is_managed,
            load_config as hermes_load_config,
            save_config,
        )
    except Exception as e:
        return f"hermes_cli not available: {e}"

    if is_managed():
        return "Hermes installation is managed; config.yaml is read-only"

    try:
        cfg = hermes_load_config()
    except Exception as e:
        return f"failed to load hermes config: {e}"

    plugins = cfg.setdefault("plugins", {})
    if not isinstance(plugins, dict):
        return "plugins section in config.yaml is not a mapping"
    ws_ckpt = plugins.setdefault("ws-ckpt", {})
    if not isinstance(ws_ckpt, dict):
        return "plugins.ws-ckpt section in config.yaml is not a mapping"
    for k, v in fields.items():
        ws_ckpt[k] = v

    try:
        save_config(cfg)
    except Exception as e:
        return f"failed to save config.yaml: {e}"
    return ""


def _resolve_workspace(args: Dict[str, Any]) -> Tuple[str, Optional[str]]:
    """Resolve workspace from args (explicit override) or config (fallback)."""
    explicit = (args.get("workspace") or "").strip()
    if explicit:
        return explicit, None
    return _require_workspace()


def handle_ws_ckpt_checkpoint(args: Dict[str, Any], **_kwargs) -> str:
    """Handle ws-ckpt-checkpoint tool call."""
    snapshot_id = (args.get("id") or "").strip()
    if not snapshot_id:
        return _err("'id' is required")

    workspace, ws_err = _resolve_workspace(args)
    if ws_err:
        return ws_err
    rejection = _reject_if_cwd_inside_workspace(workspace)
    if rejection:
        return rejection

    message = (args.get("message") or "").strip() or "manual checkpoint"

    cmd = ["ws-ckpt", "checkpoint", "-w", workspace, "-i", snapshot_id,
           "-m", message]
    success, output = _run_ws_ckpt_cmd(cmd)
    return _ok(output) if success else _err(output)


def handle_ws_ckpt_rollback(args: Dict[str, Any], **_kwargs) -> str:
    """Handle ws-ckpt-rollback tool call."""
    target = (args.get("target") or "").strip()
    if not target:
        return _err("'target' is required")

    workspace, ws_err = _resolve_workspace(args)
    if ws_err:
        return ws_err
    rejection = _reject_if_cwd_inside_workspace(workspace)
    if rejection:
        return rejection

    cmd = ["ws-ckpt", "rollback", "-w", workspace, "-s", target]
    success, output = _run_ws_ckpt_cmd(cmd)
    return _ok(output) if success else _err(output)


def handle_ws_ckpt_list(args: Dict[str, Any], **_kwargs) -> str:
    """Handle ws-ckpt-list tool call."""
    workspace, ws_err = _require_workspace()
    if ws_err:
        return ws_err
    cmd = ["ws-ckpt", "list", "-w", workspace, "--format", "table"]
    success, output = _run_ws_ckpt_cmd(cmd)
    return _ok(output) if success else _err(output)


def handle_ws_ckpt_diff(args: Dict[str, Any], **_kwargs) -> str:
    """Handle ws-ckpt-diff tool call."""
    from_id = (args.get("from") or "").strip()
    to_id = (args.get("to") or "").strip()
    if not from_id:
        return _err("'from' is required")
    if not to_id:
        return _err("'to' is required")

    workspace, ws_err = _require_workspace()
    if ws_err:
        return ws_err
    cmd = ["ws-ckpt", "diff", "-w", workspace, "--from", from_id, "--to", to_id]
    success, output = _run_ws_ckpt_cmd(cmd)
    return _ok(output) if success else _err(output)


def handle_ws_ckpt_delete(args: Dict[str, Any], **_kwargs) -> str:
    """Handle ws-ckpt-delete tool call."""
    snapshot = (args.get("snapshot") or "").strip()
    if not snapshot:
        return _err("'snapshot' is required")

    workspace, ws_err = _resolve_workspace(args)
    if ws_err:
        return ws_err
    cmd = ["ws-ckpt", "delete", "-s", snapshot, "-w", workspace, "--force"]
    success, output = _run_ws_ckpt_cmd(cmd)
    return _ok(output) if success else _err(output)


def handle_ws_ckpt_status(args: Dict[str, Any], **_kwargs) -> str:
    """Handle ws-ckpt-status tool call."""
    workspace, ws_err = _require_workspace()
    if ws_err:
        return ws_err
    cmd = ["ws-ckpt", "status", "-w", workspace, "--format", "table"]
    success, output = _run_ws_ckpt_cmd(cmd)
    return _ok(output) if success else _err(output)


# ---------------------------------------------------------------------------
# Export tuple: (name, schema, handler, emoji)
# ---------------------------------------------------------------------------

TOOLS = (
    ("ws-ckpt-config", WS_CKPT_CONFIG_SCHEMA, handle_ws_ckpt_config, "⚙️"),
    ("ws-ckpt-checkpoint", WS_CKPT_CHECKPOINT_SCHEMA, handle_ws_ckpt_checkpoint, "📸"),
    ("ws-ckpt-rollback", WS_CKPT_ROLLBACK_SCHEMA, handle_ws_ckpt_rollback, "⏪"),
    ("ws-ckpt-list", WS_CKPT_LIST_SCHEMA, handle_ws_ckpt_list, "📋"),
    ("ws-ckpt-diff", WS_CKPT_DIFF_SCHEMA, handle_ws_ckpt_diff, "🔀"),
    ("ws-ckpt-delete", WS_CKPT_DELETE_SCHEMA, handle_ws_ckpt_delete, "🗑"),
    ("ws-ckpt-status", WS_CKPT_STATUS_SCHEMA, handle_ws_ckpt_status, "📊"),
)
