"""Daemon process contract: socket permissions, shutdown, and argument guards.

These checks are about the ``agent-sec-daemon`` process itself rather than the PAP
methods it serves: the bound socket must be private (0600), SIGTERM must trigger
a cooperative exit that unlinks the socket, and the V1 systemd invocation must
resolve its socket below ``XDG_RUNTIME_DIR``.
"""

import os
import signal
import stat
import subprocess
import time


def test_bound_socket_is_private(daemon):
    mode = stat.S_IMODE(os.stat(daemon.socket_path).st_mode)
    assert mode == 0o600, f"socket mode is {oct(mode)}, expected 0o600"


def test_sigterm_cooperatively_exits_and_removes_socket(start_daemon):
    handle = start_daemon()
    socket_path = handle.socket_path
    assert socket_path.exists()
    socket_inode = os.stat(socket_path).st_ino

    handle.process.send_signal(signal.SIGTERM)
    handle.process.communicate(timeout=5.0)

    assert handle.process.returncode == 0
    # The daemon owns its socket and unlinks exactly the inode it bound.
    assert not socket_path.exists(), f"socket inode {socket_inode} was left behind"


def test_v1_service_invocation_uses_runtime_socket(daemon_bin, tmp_path):
    runtime_dir = tmp_path / "runtime"
    socket_path = runtime_dir / "agent-sec-core" / "daemon.sock"
    socket_path.parent.mkdir(parents=True)
    environment = os.environ.copy()
    environment["XDG_RUNTIME_DIR"] = str(runtime_dir)
    process = subprocess.Popen(
        [daemon_bin, "serve", "--policy-admin-uid", str(os.getuid())],
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    deadline = time.monotonic() + 5.0
    try:
        while time.monotonic() < deadline and not socket_path.exists():
            assert process.poll() is None, process.communicate()[1]
            time.sleep(0.02)
        assert socket_path.exists()
    finally:
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
        process.communicate(timeout=5.0)
    assert process.returncode == 0
    assert not socket_path.exists()


def test_missing_socket_and_runtime_directory_fails(daemon_bin):
    environment = os.environ.copy()
    environment.pop("XDG_RUNTIME_DIR", None)
    result = subprocess.run(
        [daemon_bin],
        env=environment,
        capture_output=True,
        text=True,
        timeout=10.0,
        check=False,
    )
    assert result.returncode != 0
    assert result.stderr != ""
