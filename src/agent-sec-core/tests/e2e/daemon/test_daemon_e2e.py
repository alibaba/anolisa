"""E2E tests for the agent-sec daemon process."""

import json
import os
import shutil
import signal
import socket
import stat
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import pytest


@dataclass
class DaemonOutput:
    stdout: str
    stderr: str
    returncode: int


@pytest.fixture
def daemon_command() -> list[str]:
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

    pytest.skip("agent-sec-daemon is not installed and daemon module is not importable")


def test_daemon_health_over_unix_socket(
    daemon_command: list[str], tmp_path: Path
) -> None:
    socket_path = tmp_path / "runtime" / "daemon.sock"
    process = _start_daemon(daemon_command, socket_path, tmp_path)
    output: DaemonOutput | None = None

    try:
        response = _call_daemon(
            socket_path,
            {"id": "e2e-health", "method": "daemon.health"},
        )

        assert response["id"] == "e2e-health"
        assert response["ok"] is True
        assert response["exit_code"] == 0
        assert response.get("error") is None
        assert response["data"]["status"] == "ok"
        assert response["data"]["socket"] == str(socket_path)
        assert response["data"]["prompt_scan"]["status"] == "pending"
        assert response["data"]["prompt_scan"]["loaded"] is False
        assert response["data"]["jobs"] == []
        assert "inflight" in response["data"]["queues"]
        assert "queued" in response["data"]["queues"]
        assert stat.S_IMODE(socket_path.parent.stat().st_mode) == 0o700
        assert stat.S_IMODE(socket_path.stat().st_mode) == 0o600
    finally:
        output = _stop_daemon(process)

    assert not socket_path.exists()
    assert output.returncode == 0
    assert _has_request_log(output, "e2e-health", "daemon.health")


def test_daemon_uses_xdg_runtime_dir_by_default(
    daemon_command: list[str], tmp_path: Path
) -> None:
    xdg_runtime_dir = tmp_path / "xdg"
    socket_path = xdg_runtime_dir / "agent-sec-core" / "daemon.sock"
    process = _start_daemon(
        daemon_command,
        socket_path,
        tmp_path,
        use_default_socket=True,
        xdg_runtime_dir=xdg_runtime_dir,
    )

    try:
        response = _call_daemon(
            socket_path,
            {"id": "e2e-default-socket", "method": "daemon.health"},
        )
    finally:
        output = _stop_daemon(process)

    assert response["ok"] is True
    assert response["data"]["socket"] == str(socket_path)
    assert not socket_path.exists()
    assert output.returncode == 0


def test_daemon_unknown_method_returns_structured_error(
    daemon_command: list[str], tmp_path: Path
) -> None:
    socket_path = tmp_path / "runtime" / "daemon.sock"
    process = _start_daemon(daemon_command, socket_path, tmp_path)

    try:
        response = _call_daemon(
            socket_path,
            {"id": "e2e-unknown", "method": "unknown.method"},
        )
    finally:
        output = _stop_daemon(process)

    assert response["id"] == "e2e-unknown"
    assert response["ok"] is False
    assert response["exit_code"] == 1
    assert response["stderr"] == "unknown daemon method: unknown.method"
    assert response["error"] == {
        "code": "unknown_method",
        "message": "unknown daemon method: unknown.method",
    }
    assert output.returncode == 0
    assert _has_request_log(output, "e2e-unknown", "unknown.method")


def test_daemon_returns_busy_when_connection_limit_is_exhausted(
    daemon_command: list[str], tmp_path: Path
) -> None:
    socket_path = tmp_path / "runtime" / "daemon.sock"
    process = _start_daemon(
        daemon_command,
        socket_path,
        tmp_path,
        max_connections=0,
        wait_for_health=False,
    )

    try:
        _wait_for_socket(socket_path, process)
        response = _call_daemon(
            socket_path,
            {"id": "e2e-busy", "method": "daemon.health"},
        )
    finally:
        output = _stop_daemon(process)

    assert response["ok"] is False
    assert response["exit_code"] == 1
    assert response["error"] == {"code": "busy", "message": "daemon is busy"}
    assert output.returncode == 0


def test_daemon_idle_client_times_out_and_releases_connection(
    daemon_command: list[str], tmp_path: Path
) -> None:
    socket_path = tmp_path / "runtime" / "daemon.sock"
    process = _start_daemon(
        daemon_command,
        socket_path,
        tmp_path,
        max_connections=1,
        request_read_timeout_ms=100,
    )

    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as idle_socket:
            idle_socket.settimeout(5)
            idle_socket.connect(str(socket_path))
            timeout_response = json.loads(_read_response(idle_socket).decode("utf-8"))

        health_response = _call_daemon(
            socket_path,
            {"id": "e2e-after-idle-timeout", "method": "daemon.health"},
        )
    finally:
        output = _stop_daemon(process)

    assert timeout_response["ok"] is False
    assert timeout_response["error"]["code"] == "timeout"
    assert health_response["ok"] is True
    assert output.returncode == 0


def test_daemon_sigterm_graceful_shutdown(
    daemon_command: list[str], tmp_path: Path
) -> None:
    socket_path = tmp_path / "runtime" / "daemon.sock"
    process = _start_daemon(daemon_command, socket_path, tmp_path)

    output = _stop_daemon(process, stop_signal=signal.SIGTERM)

    assert output.returncode == 0
    assert not socket_path.exists()


def _start_daemon(
    daemon_command: list[str],
    socket_path: Path,
    tmp_path: Path,
    max_connections: int = 64,
    request_read_timeout_ms: int = 5000,
    wait_for_health: bool = True,
    use_default_socket: bool = False,
    xdg_runtime_dir: Path | None = None,
) -> subprocess.Popen[str]:
    env = os.environ.copy()
    env.pop("AGENT_SEC_DAEMON_SOCKET", None)
    env["AGENT_SEC_DATA_DIR"] = str(tmp_path / "data")
    env["PYTHONUNBUFFERED"] = "1"
    if xdg_runtime_dir is not None:
        env["XDG_RUNTIME_DIR"] = str(xdg_runtime_dir)

    command = [*daemon_command, "serve"]
    if not use_default_socket:
        command.extend(["--socket", str(socket_path)])
    command.extend(["--max-connections", str(max_connections)])
    command.extend(["--request-read-timeout-ms", str(request_read_timeout_ms)])

    process = subprocess.Popen(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        env=env,
    )
    if wait_for_health:
        _wait_for_health(socket_path, process)
    return process


def _stop_daemon(
    process: subprocess.Popen[str],
    stop_signal: signal.Signals = signal.SIGINT,
) -> DaemonOutput:
    if process.poll() is None:
        process.send_signal(stop_signal)

    try:
        stdout, stderr = process.communicate(timeout=10)
    except subprocess.TimeoutExpired:
        process.terminate()
        try:
            stdout, stderr = process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            stdout, stderr = process.communicate(timeout=5)

    return DaemonOutput(
        stdout=stdout,
        stderr=stderr,
        returncode=0 if process.returncode is None else process.returncode,
    )


def _wait_for_health(socket_path: Path, process: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 10
    last_error: Exception | None = None

    while time.monotonic() < deadline:
        _assert_process_running(process)
        if socket_path.exists():
            try:
                response = _call_daemon(
                    socket_path,
                    {"id": "e2e-wait-health", "method": "daemon.health"},
                )
            except OSError as exc:
                last_error = exc
            else:
                if response.get("ok") is True:
                    return
        time.sleep(0.05)

    raise AssertionError(f"daemon did not become healthy; last_error={last_error!r}")


def _wait_for_socket(socket_path: Path, process: subprocess.Popen[str]) -> None:
    deadline = time.monotonic() + 10

    while time.monotonic() < deadline:
        _assert_process_running(process)
        if socket_path.exists():
            return
        time.sleep(0.05)

    raise AssertionError(f"daemon socket was not created: {socket_path}")


def _assert_process_running(process: subprocess.Popen[str]) -> None:
    if process.poll() is None:
        return

    stdout, stderr = process.communicate(timeout=1)
    raise AssertionError(
        f"daemon exited before test request; returncode={process.returncode}; "
        f"stdout={stdout!r}; stderr={stderr!r}"
    )


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


def _has_request_log(output: DaemonOutput, request_id: str, method: str | None) -> bool:
    for line in f"{output.stdout}\n{output.stderr}".splitlines():
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if (
            payload.get("event") == "daemon_request_completed"
            and payload.get("request_id") == request_id
            and payload.get("method") == method
            and "latency_ms" in payload
            and "bytes_in" in payload
            and "bytes_out" in payload
        ):
            return True
    return False
