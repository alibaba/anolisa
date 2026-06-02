"""Daemon runtime state and runtime path helpers."""

import os
import stat
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from agent_sec_cli.daemon.errors import DaemonRuntimePathError
from agent_sec_cli.daemon.jobs import JobManager

SOCKET_ENV = "AGENT_SEC_DAEMON_SOCKET"
RUNTIME_SUBDIR = "agent-sec-core"
SOCKET_FILENAME = "daemon.sock"
LOCK_FILENAME = "daemon.lock"


@dataclass
class PromptRuntimeState:
    """Prompt runtime state exposed by health without initializing the model."""

    status: str = "pending"
    model: str | None = None
    loaded: bool = False
    last_error: str | None = None

    def to_dict(self) -> dict[str, Any]:
        """Return a JSON-serializable prompt state."""
        return {
            "status": self.status,
            "model": self.model,
            "loaded": self.loaded,
            "last_error": self.last_error,
        }


@dataclass
class QueueState:
    """Lightweight request queue counters for health."""

    inflight: int = 0
    queued: int = 0

    def to_dict(self) -> dict[str, int]:
        """Return a JSON-serializable queue state."""
        return {"inflight": self.inflight, "queued": self.queued}


@dataclass
class DaemonRuntime:
    """Shared daemon runtime state for request handlers."""

    socket_path: Path
    started_monotonic: float = field(default_factory=time.monotonic)
    status: str = "ok"
    prompt_scan: PromptRuntimeState = field(default_factory=PromptRuntimeState)
    queues: QueueState = field(default_factory=QueueState)
    jobs: JobManager = field(default_factory=JobManager)

    def uptime_seconds(self) -> float:
        """Return daemon process uptime in seconds."""
        return max(0.0, time.monotonic() - self.started_monotonic)

    def begin_request(self) -> None:
        """Increment in-flight request count."""
        self.queues.inflight += 1

    def end_request(self) -> None:
        """Decrement in-flight request count."""
        if self.queues.inflight > 0:
            self.queues.inflight -= 1

    def mark_stopping(self) -> None:
        """Mark runtime as stopping for health responses."""
        self.status = "stopping"


def resolve_socket_path(
    socket_path: str | Path | None = None, use_env: bool = True
) -> Path:
    """Resolve the daemon Unix socket path."""
    if socket_path is not None:
        return Path(socket_path)

    if use_env:
        env_socket_path = os.environ.get(SOCKET_ENV)
        if env_socket_path:
            return Path(env_socket_path)

    xdg_runtime_dir = os.environ.get("XDG_RUNTIME_DIR")
    if not xdg_runtime_dir:
        raise DaemonRuntimePathError("XDG_RUNTIME_DIR is required for agent-sec daemon")

    return Path(xdg_runtime_dir) / RUNTIME_SUBDIR / SOCKET_FILENAME


def lock_path_for_socket(socket_path: Path) -> Path:
    """Return the single-instance lock path for a socket path."""
    return socket_path.with_name(LOCK_FILENAME)


def ensure_runtime_directory(socket_path: Path) -> None:
    """Create and validate the daemon runtime directory with mode 0700."""
    runtime_dir = socket_path.parent
    created_runtime_dir = False

    try:
        runtime_lstat = runtime_dir.lstat()
    except FileNotFoundError:
        try:
            runtime_dir.mkdir(mode=0o700, parents=True, exist_ok=False)
            created_runtime_dir = True
        except FileExistsError:
            pass
        runtime_lstat = runtime_dir.lstat()

    if stat.S_ISLNK(runtime_lstat.st_mode):
        raise DaemonRuntimePathError(
            f"runtime directory must not be a symlink: {runtime_dir}"
        )
    if not stat.S_ISDIR(runtime_lstat.st_mode):
        raise DaemonRuntimePathError(f"runtime path is not a directory: {runtime_dir}")

    runtime_stat = runtime_dir.stat()
    if not stat.S_ISDIR(runtime_stat.st_mode):
        raise DaemonRuntimePathError(f"runtime path is not a directory: {runtime_dir}")
    if runtime_stat.st_uid != os.getuid():
        raise DaemonRuntimePathError(
            f"runtime directory is not owned by current user: {runtime_dir}"
        )

    if created_runtime_dir:
        os.chmod(runtime_dir, 0o700)
        runtime_stat = runtime_dir.stat()

    runtime_mode = stat.S_IMODE(runtime_stat.st_mode)
    if runtime_mode != 0o700:
        raise DaemonRuntimePathError(
            f"runtime directory must be mode 0700: {runtime_dir}"
        )
