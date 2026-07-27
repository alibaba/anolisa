"""Execution-path fixtures for prompt-scanner e2e tests."""

import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import time
from collections.abc import Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any, TextIO

import pytest
from agent_sec_cli.daemon.env import DAEMON_DISABLED_ENV, SOCKET_ENV
from agent_sec_cli.telemetry.config import TELEMETRY_LOG_PATH_ENV

DATA_DIR_ENV = "AGENT_SEC_DATA_DIR"
PROMPT_PRELOAD_ENV = "AGENT_SEC_DAEMON_PROMPT_PRELOAD"
READY_TIMEOUT_ENV = "AGENT_SEC_PROMPT_E2E_READY_TIMEOUT_SECONDS"
DEFAULT_READY_TIMEOUT_SECONDS = 600
PROMPT_READY_POLL_INTERVAL_SECONDS = 5.0
READY_PROGRESS_INTERVAL_SECONDS = 10.0


@dataclass
class DaemonOutput:
    stdout: str
    stderr: str
    returncode: int


@dataclass
class DaemonProcess:
    process: subprocess.Popen[str]
    stdout_path: Path
    stderr_path: Path
    stdout_file: TextIO
    stderr_file: TextIO


@dataclass(frozen=True)
class PromptScanExecutionContext:
    execution_path: str
    socket_path: Path
    data_dir: Path
    telemetry_path: Path


def _resolve_daemon_command() -> list[str]:
    """Return the installed daemon binary or a source-tree module fallback."""
    daemon_bin = shutil.which("agent-sec-daemon")
    if daemon_bin:
        return [daemon_bin]

    result = subprocess.run(
        [sys.executable, "-c", "import agent_sec_cli.daemon.server"],
        capture_output=True,
        check=False,
        text=True,
        timeout=10,
    )
    if result.returncode == 0:
        return [sys.executable, "-m", "agent_sec_cli.daemon.server"]

    pytest.fail(
        "agent-sec-daemon is unavailable and agent_sec_cli.daemon.server "
        f"cannot be imported: {result.stderr}"
    )


@pytest.fixture(
    scope="module",
    params=("daemon", "middleware"),
    ids=("daemon", "middleware"),
    autouse=True,
)
def prompt_scan_execution_path(
    request: pytest.FixtureRequest,
    tmp_path_factory: pytest.TempPathFactory,
) -> Iterator[PromptScanExecutionContext]:
    """Run the CLI e2e suite against daemon and explicit local middleware paths."""
    execution_path = str(request.param)
    tmp_path = tmp_path_factory.mktemp(f"prompt_scan_{execution_path}")
    socket_path = tmp_path / "runtime" / "daemon.sock"
    data_dir = tmp_path / "data"
    telemetry_path = data_dir / "telemetry.jsonl"
    telemetry_path.parent.mkdir(parents=True, exist_ok=True)
    telemetry_path.write_text("", encoding="utf-8")

    saved_env = {
        SOCKET_ENV: os.environ.get(SOCKET_ENV),
        DAEMON_DISABLED_ENV: os.environ.get(DAEMON_DISABLED_ENV),
        DATA_DIR_ENV: os.environ.get(DATA_DIR_ENV),
        TELEMETRY_LOG_PATH_ENV: os.environ.get(TELEMETRY_LOG_PATH_ENV),
    }

    daemon: DaemonProcess | None = None
    output: DaemonOutput | None = None
    try:
        if execution_path == "daemon":
            daemon = _start_daemon(
                _resolve_daemon_command(),
                socket_path,
                data_dir,
                telemetry_path,
            )
            _wait_for_prompt_scan_ready(socket_path, daemon)
            os.environ[SOCKET_ENV] = str(socket_path)
            os.environ.pop(DAEMON_DISABLED_ENV, None)
        else:
            _print_progress(
                "using local middleware path " f"with {DAEMON_DISABLED_ENV}=1"
            )
            os.environ.pop(SOCKET_ENV, None)
            os.environ[DAEMON_DISABLED_ENV] = "1"

        os.environ[DATA_DIR_ENV] = str(data_dir)
        os.environ[TELEMETRY_LOG_PATH_ENV] = str(telemetry_path)
        yield PromptScanExecutionContext(
            execution_path=execution_path,
            socket_path=socket_path,
            data_dir=data_dir,
            telemetry_path=telemetry_path,
        )
    finally:
        if daemon is not None:
            output = _stop_daemon(daemon)
        for key, value in saved_env.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value

    if output is not None:
        assert output.returncode == 0


def _start_daemon(
    daemon_command: list[str],
    socket_path: Path,
    data_dir: Path,
    telemetry_path: Path,
) -> DaemonProcess:
    env = os.environ.copy()
    env.pop(SOCKET_ENV, None)
    env.pop(DAEMON_DISABLED_ENV, None)
    env[DATA_DIR_ENV] = str(data_dir)
    env[TELEMETRY_LOG_PATH_ENV] = str(telemetry_path)
    env[PROMPT_PRELOAD_ENV] = "1"
    env["PYTHONUNBUFFERED"] = "1"

    log_dir = data_dir.parent / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    stdout_path = log_dir / "daemon.stdout.log"
    stderr_path = log_dir / "daemon.stderr.log"
    stdout_file = stdout_path.open("w+", encoding="utf-8")
    stderr_file = stderr_path.open("w+", encoding="utf-8")

    command = [
        *daemon_command,
        "serve",
        "--socket",
        str(socket_path),
        "--request-read-timeout-ms",
        "30000",
    ]
    process = subprocess.Popen(
        command,
        stdout=stdout_file,
        stderr=stderr_file,
        text=True,
        env=env,
    )
    daemon = DaemonProcess(
        process=process,
        stdout_path=stdout_path,
        stderr_path=stderr_path,
        stdout_file=stdout_file,
        stderr_file=stderr_file,
    )
    _print_progress(
        "started daemon "
        f"pid={process.pid} socket={socket_path} "
        f"stdout={stdout_path} stderr={stderr_path}"
    )
    _wait_for_health(socket_path, daemon)
    return daemon


def _stop_daemon(
    daemon: DaemonProcess,
    stop_signal: signal.Signals = signal.SIGINT,
) -> DaemonOutput:
    process = daemon.process
    if process.poll() is None:
        process.send_signal(stop_signal)

    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)

    daemon.stdout_file.close()
    daemon.stderr_file.close()

    return DaemonOutput(
        stdout=_read_log_file(daemon.stdout_path),
        stderr=_read_log_file(daemon.stderr_path),
        returncode=0 if process.returncode is None else process.returncode,
    )


def _wait_for_health(socket_path: Path, daemon: DaemonProcess) -> None:
    deadline = time.monotonic() + 10
    last_error: Exception | None = None
    _print_progress("waiting for daemon.health")

    while time.monotonic() < deadline:
        _assert_process_running(daemon)
        if socket_path.exists():
            try:
                response = _call_daemon(
                    socket_path,
                    {"id": "prompt-e2e-wait-health", "method": "daemon.health"},
                )
            except OSError as exc:
                last_error = exc
            else:
                if response.get("ok") is True:
                    _print_progress("daemon.health is ready")
                    return
        time.sleep(0.5)

    raise AssertionError(f"daemon did not become healthy; last_error={last_error!r}")


def _wait_for_prompt_scan_ready(
    socket_path: Path,
    daemon: DaemonProcess,
) -> None:
    timeout_seconds = int(
        os.environ.get(READY_TIMEOUT_ENV, str(DEFAULT_READY_TIMEOUT_SECONDS))
    )
    deadline = time.monotonic() + timeout_seconds
    last_state: dict[str, Any] | None = None
    last_error: Exception | None = None
    started_at = time.monotonic()
    next_progress_at = 0.0
    last_progress_key: tuple[Any, ...] | None = None

    _print_progress(
        "waiting for prompt scanner model ready " f"timeout_seconds={timeout_seconds}"
    )

    while time.monotonic() < deadline:
        _assert_process_running(daemon)
        try:
            response = _call_daemon(
                socket_path,
                {
                    "id": "prompt-e2e-wait-prompt-ready",
                    "method": "daemon.health",
                },
            )
        except OSError as exc:
            last_error = exc
            now = time.monotonic()
            if now >= next_progress_at:
                elapsed = now - started_at
                _print_progress(
                    "waiting for prompt scanner ready "
                    f"elapsed={elapsed:.1f}s last_error={exc!r}"
                )
                next_progress_at = now + READY_PROGRESS_INTERVAL_SECONDS
            time.sleep(PROMPT_READY_POLL_INTERVAL_SECONDS)
            continue

        if response.get("ok") is not True:
            error = response.get("error") or {}
            if error.get("code") == "busy":
                last_state = {"status": "busy"}
                now = time.monotonic()
                if now >= next_progress_at:
                    elapsed = now - started_at
                    _print_progress(
                        "waiting for prompt scanner ready "
                        f"elapsed={elapsed:.1f}s status=busy"
                    )
                    next_progress_at = now + READY_PROGRESS_INTERVAL_SECONDS
                time.sleep(PROMPT_READY_POLL_INTERVAL_SECONDS)
                continue
            raise AssertionError(f"daemon health request failed: {response!r}")

        prompt_state = response["data"]["prompt_scan"]
        last_state = prompt_state
        now = time.monotonic()
        progress_key = (
            prompt_state.get("status"),
            prompt_state.get("loaded"),
            prompt_state.get("model"),
            prompt_state.get("last_error"),
        )
        if progress_key != last_progress_key or now >= next_progress_at:
            elapsed = now - started_at
            _print_progress(
                "waiting for prompt scanner ready "
                f"elapsed={elapsed:.1f}s status={prompt_state.get('status')} "
                f"loaded={prompt_state.get('loaded')} "
                f"model={prompt_state.get('model')} "
                f"last_error={prompt_state.get('last_error')}"
            )
            last_progress_key = progress_key
            next_progress_at = now + READY_PROGRESS_INTERVAL_SECONDS

        if prompt_state.get("status") == "ready" and prompt_state.get("loaded") is True:
            elapsed = time.monotonic() - started_at
            _print_progress(f"prompt scanner model is ready elapsed={elapsed:.1f}s")
            return
        if prompt_state.get("status") == "degraded":
            raise AssertionError(
                "daemon prompt scanner preload failed; " f"state={prompt_state!r}"
            )
        time.sleep(PROMPT_READY_POLL_INTERVAL_SECONDS)

    raise AssertionError(
        "daemon prompt scanner did not become ready within "
        f"{timeout_seconds}s; last_state={last_state!r}; last_error={last_error!r}"
    )


def _assert_process_running(daemon: DaemonProcess) -> None:
    process = daemon.process
    if process.poll() is None:
        return

    stdout = _read_log_file(daemon.stdout_path)
    stderr = _read_log_file(daemon.stderr_path)
    raise AssertionError(
        f"daemon exited before prompt-scanner e2e; returncode={process.returncode}; "
        f"stdout={stdout!r}; stderr={stderr!r}"
    )


def _read_log_file(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""


def _print_progress(message: str) -> None:
    print(f"[prompt-scanner-e2e] {message}", flush=True)


def _call_daemon(socket_path: Path, request: dict[str, Any]) -> dict[str, Any]:
    raw_request = json.dumps(request, separators=(",", ":")).encode("utf-8") + b"\n"
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client_socket:
        client_socket.settimeout(5)
        client_socket.connect(str(socket_path))
        client_socket.sendall(raw_request)
        raw_response = _read_response(client_socket)

    response = json.loads(raw_response.decode("utf-8"))
    assert isinstance(response, dict)
    return response


def _read_response(client_socket: socket.socket) -> bytes:
    chunks: list[bytes] = []
    total_bytes = 0

    while True:
        chunk = client_socket.recv(4096)
        if not chunk:
            break
        chunks.append(chunk)
        total_bytes += len(chunk)
        if total_bytes > 4 * 1024 * 1024:
            raise AssertionError("daemon response exceeded e2e read limit")
        if b"\n" in chunk:
            break

    if not chunks:
        raise AssertionError("daemon returned an empty response")

    raw_response, _separator, _remaining = b"".join(chunks).partition(b"\n")
    return raw_response
