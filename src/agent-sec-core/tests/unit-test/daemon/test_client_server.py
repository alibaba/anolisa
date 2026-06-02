"""Tests for daemon client/server integration."""

import asyncio
import contextlib
import json
import logging
import os
import socket
import stat
import sys
import time
from pathlib import Path

from agent_sec_cli.daemon.client import DaemonClient
from agent_sec_cli.daemon.errors import (
    DaemonProtocolError,
    DaemonRuntimePathError,
)
from agent_sec_cli.daemon.health import (
    build_health_snapshot,
    create_default_registry,
)
from agent_sec_cli.daemon.protocol import (
    DaemonRequest,
    DaemonResponse,
    parse_response_line,
    serialize_request,
    success_response,
)
from agent_sec_cli.daemon.registry import (
    HandlerResult,
    MethodRegistry,
    MethodSpec,
)
from agent_sec_cli.daemon.runtime import DaemonRuntime
from agent_sec_cli.daemon.server import (
    DaemonServer,
    _log_request_completion,
    prepare_socket_path,
)


def test_client_uses_env_socket_override(monkeypatch, tmp_path: Path):
    socket_path = tmp_path / "runtime" / "daemon.sock"
    monkeypatch.setenv("AGENT_SEC_DAEMON_SOCKET", str(socket_path))

    client = DaemonClient()

    assert client.socket_path == socket_path


def test_daemon_client_rejects_oversized_response(tmp_path: Path):
    server_socket, client_socket = socket.socketpair()
    try:
        client = DaemonClient(
            socket_path=tmp_path / "unused.sock", max_response_bytes=8
        )
        server_socket.sendall(
            b'{"id":"req-1","ok":true,"data":{},"stdout":"","stderr":"","exit_code":0}\n'
        )

        try:
            client._read_response_line(client_socket)
        except DaemonProtocolError as exc:
            assert "exceeds byte limit" in str(exc)
        else:
            raise AssertionError("expected oversized daemon response to fail")
    finally:
        server_socket.close()
        client_socket.close()


def test_daemon_write_response_closes_writer_when_drain_is_cancelled(
    tmp_path: Path,
):
    class BlockingDrainWriter:
        def __init__(self) -> None:
            self.drain_started = asyncio.Event()
            self.closed = False
            self.wait_closed_called = False
            self.wrote = False

        def write(self, _data: bytes) -> None:
            self.wrote = True

        async def drain(self) -> None:
            self.drain_started.set()
            await asyncio.Event().wait()

        def close(self) -> None:
            self.closed = True

        async def wait_closed(self) -> None:
            self.wait_closed_called = True

    async def scenario():
        writer = BlockingDrainWriter()
        server = DaemonServer(socket_path=tmp_path / "runtime" / "daemon.sock")
        write_task = asyncio.create_task(
            server._write_response(writer, success_response("req-cancel-write"))
        )
        await asyncio.wait_for(writer.drain_started.wait(), timeout=0.5)

        write_task.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await write_task

        return writer

    writer = asyncio.run(scenario())

    assert writer.wrote is True
    assert writer.closed is True
    assert writer.wait_closed_called is True


def test_daemon_server_uses_default_job_registration(monkeypatch, tmp_path: Path):
    registered_managers = []

    def fake_register_default_jobs(job_manager):
        registered_managers.append(job_manager)

    monkeypatch.setattr(
        "agent_sec_cli.daemon.server.register_default_jobs",
        fake_register_default_jobs,
    )

    server = DaemonServer(socket_path=tmp_path / "runtime" / "daemon.sock")

    assert registered_managers == [server.runtime.jobs]


def test_daemon_client_calls_health_over_temp_socket(tmp_path: Path):
    async def scenario():
        socket_path = tmp_path / "runtime" / "daemon.sock"
        server = DaemonServer(socket_path=socket_path)
        await server.start()
        try:
            client = DaemonClient(socket_path=socket_path)
            response = await asyncio.to_thread(
                client.call,
                "daemon.health",
                request_id="req-health",
            )
            runtime_dir_mode = stat.S_IMODE(socket_path.parent.stat().st_mode)
            socket_mode = stat.S_IMODE(socket_path.stat().st_mode)
        finally:
            await server.stop()

        return response, runtime_dir_mode, socket_mode

    response, runtime_dir_mode, socket_mode = asyncio.run(scenario())

    assert response.id == "req-health"
    assert response.ok is True
    assert response.exit_code == 0
    assert response.data["status"] == "ok"
    assert response.data["prompt_scan"]["status"] == "pending"
    assert response.data["socket"].endswith("daemon.sock")
    assert runtime_dir_mode == 0o700
    assert socket_mode == 0o600


def test_daemon_start_closes_bound_server_when_chmod_fails(monkeypatch, tmp_path: Path):
    async def scenario():
        socket_path = tmp_path / "runtime" / "daemon.sock"
        server = DaemonServer(socket_path=socket_path)
        original_chmod = os.chmod

        def fail_socket_chmod(path: str | Path, mode: int) -> None:
            if Path(path) == socket_path:
                raise PermissionError("forced chmod failure")
            original_chmod(path, mode)

        monkeypatch.setattr("agent_sec_cli.daemon.server.os.chmod", fail_socket_chmod)

        try:
            await server.start()
        except PermissionError:
            pass
        else:
            raise AssertionError("expected chmod failure during daemon start")

        rebound = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            rebound.bind(str(socket_path))
        finally:
            rebound.close()
            with contextlib.suppress(FileNotFoundError):
                socket_path.unlink()

        return server._server, socket_path.exists()

    bound_server, socket_exists = asyncio.run(scenario())

    assert bound_server is None
    assert socket_exists is False


def test_daemon_server_returns_busy_when_connection_limit_is_reached(tmp_path: Path):
    async def scenario():
        socket_path = tmp_path / "runtime" / "daemon.sock"
        handler_started = asyncio.Event()
        handler_release = asyncio.Event()

        async def slow_handler(
            _request: DaemonRequest, _runtime: DaemonRuntime
        ) -> HandlerResult:
            handler_started.set()
            await handler_release.wait()
            return HandlerResult(data={"done": True})

        registry = MethodRegistry()
        registry.register(
            MethodSpec(method="slow", handler=slow_handler, lifecycle="test")
        )
        registry.register(
            MethodSpec(
                method="daemon.health",
                handler=lambda _request, _runtime: HandlerResult(data={}),
                lifecycle="admin",
            )
        )
        server = DaemonServer(
            socket_path=socket_path, registry=registry, max_connections=1
        )
        await server.start()
        try:
            first_response_task = asyncio.create_task(
                _send_daemon_request(
                    socket_path, DaemonRequest(id="req-slow", method="slow")
                )
            )
            await asyncio.wait_for(handler_started.wait(), timeout=0.5)

            busy_response = await _send_daemon_request(
                socket_path,
                DaemonRequest(id="req-busy", method="daemon.health"),
            )

            handler_release.set()
            first_response = await asyncio.wait_for(first_response_task, timeout=0.5)
        finally:
            await server.stop()

        return busy_response, first_response

    busy_response, first_response = asyncio.run(scenario())

    assert busy_response.ok is False
    assert busy_response.error is not None
    assert busy_response.error["code"] == "busy"
    assert first_response.ok is True
    assert first_response.id == "req-slow"


def test_daemon_server_rejects_oversized_handler_response(tmp_path: Path, caplog):
    caplog.set_level(logging.INFO, logger="agent-sec-core.daemon")

    async def scenario():
        socket_path = tmp_path / "runtime" / "daemon.sock"

        async def large_handler(
            _request: DaemonRequest, _runtime: DaemonRuntime
        ) -> HandlerResult:
            return HandlerResult(data={"blob": "x" * 256})

        registry = MethodRegistry()
        registry.register(
            MethodSpec(method="large", handler=large_handler, lifecycle="test")
        )
        server = DaemonServer(
            socket_path=socket_path,
            registry=registry,
            max_response_bytes=220,
        )
        await server.start()
        try:
            response = await _send_daemon_request(
                socket_path, DaemonRequest(id="req-large", method="large")
            )
        finally:
            await server.stop()

        return response

    response = asyncio.run(scenario())

    assert response.id == "req-large"
    assert response.ok is False
    assert response.error is not None
    assert response.error["code"] == "payload_too_large"
    assert response.stderr == "response payload exceeds 220 bytes"

    matching_logs = [
        json.loads(record.message)
        for record in caplog.records
        if record.name == "agent-sec-core.daemon"
        and _is_json(record.message)
        and json.loads(record.message).get("request_id") == "req-large"
    ]
    assert matching_logs
    assert matching_logs[-1]["ok"] is False
    assert matching_logs[-1]["error_code"] == "payload_too_large"


def test_idle_request_read_times_out_and_releases_connection(tmp_path: Path):
    async def scenario():
        socket_path = tmp_path / "runtime" / "daemon.sock"
        server = DaemonServer(
            socket_path=socket_path,
            max_connections=1,
            request_read_timeout_ms=25,
        )
        await server.start()
        try:
            reader, writer = await asyncio.open_unix_connection(str(socket_path))
            timeout_line = await asyncio.wait_for(reader.readline(), timeout=0.5)
            writer.close()
            await writer.wait_closed()

            health_response = await _send_daemon_request(
                socket_path,
                DaemonRequest(id="req-after-idle-timeout", method="daemon.health"),
            )
        finally:
            await server.stop()

        return parse_response_line(timeout_line), health_response

    timeout_response, health_response = asyncio.run(scenario())

    assert timeout_response.ok is False
    assert timeout_response.error is not None
    assert timeout_response.error["code"] == "timeout"
    assert timeout_response.stderr == "daemon request timed out after 25 ms"
    assert health_response.ok is True
    assert health_response.id == "req-after-idle-timeout"


def test_partial_request_read_times_out_and_releases_connection(tmp_path: Path):
    async def scenario():
        socket_path = tmp_path / "runtime" / "daemon.sock"
        server = DaemonServer(
            socket_path=socket_path,
            max_connections=1,
            request_read_timeout_ms=25,
        )
        await server.start()
        try:
            reader, writer = await asyncio.open_unix_connection(str(socket_path))
            writer.write(b'{"id":"partial"')
            await writer.drain()
            timeout_line = await asyncio.wait_for(reader.readline(), timeout=0.5)
            writer.close()
            await writer.wait_closed()

            health_response = await _send_daemon_request(
                socket_path,
                DaemonRequest(id="req-after-partial-timeout", method="daemon.health"),
            )
        finally:
            await server.stop()

        return parse_response_line(timeout_line), health_response

    timeout_response, health_response = asyncio.run(scenario())

    assert timeout_response.ok is False
    assert timeout_response.error is not None
    assert timeout_response.error["code"] == "timeout"
    assert health_response.ok is True
    assert health_response.id == "req-after-partial-timeout"


def test_bad_request_does_not_steal_concurrent_inflight_counter(tmp_path: Path):
    """Parse-time failures must not decrement another request's inflight slot."""

    async def scenario():
        socket_path = tmp_path / "runtime" / "daemon.sock"
        handler_started = asyncio.Event()
        handler_release = asyncio.Event()
        observed_inflight: list[int] = []

        async def slow_handler(
            _request: DaemonRequest, runtime: DaemonRuntime
        ) -> HandlerResult:
            handler_started.set()
            await handler_release.wait()
            observed_inflight.append(runtime.queues.inflight)
            return HandlerResult(data={"done": True})

        registry = MethodRegistry()
        registry.register(
            MethodSpec(method="slow", handler=slow_handler, lifecycle="test")
        )
        server = DaemonServer(socket_path=socket_path, registry=registry)
        await server.start()
        try:
            slow_task = asyncio.create_task(
                _send_daemon_request(
                    socket_path, DaemonRequest(id="req-slow", method="slow")
                )
            )
            await asyncio.wait_for(handler_started.wait(), timeout=0.5)

            reader, writer = await asyncio.open_unix_connection(str(socket_path))
            writer.write(b"{bad-json}\n")
            await writer.drain()
            bad_line = await reader.readline()
            writer.close()
            await writer.wait_closed()

            bad_response = parse_response_line(bad_line)

            handler_release.set()
            slow_response = await asyncio.wait_for(slow_task, timeout=0.5)
        finally:
            await server.stop()

        return bad_response, slow_response, observed_inflight

    bad_response, slow_response, observed_inflight = asyncio.run(scenario())

    assert bad_response.ok is False
    assert bad_response.error is not None
    assert bad_response.error["code"] == "bad_request"
    assert slow_response.ok is True
    assert slow_response.id == "req-slow"
    assert observed_inflight == [1]


def test_daemon_server_stop_drains_inflight_request(tmp_path: Path):
    async def scenario():
        socket_path = tmp_path / "runtime" / "daemon.sock"
        handler_started = asyncio.Event()
        handler_release = asyncio.Event()

        async def slow_handler(
            _request: DaemonRequest, _runtime: DaemonRuntime
        ) -> HandlerResult:
            handler_started.set()
            await handler_release.wait()
            return HandlerResult(data={"done": True})

        registry = MethodRegistry()
        registry.register(
            MethodSpec(method="slow", handler=slow_handler, lifecycle="test")
        )
        server = DaemonServer(socket_path=socket_path, registry=registry)
        await server.start()

        response_task = asyncio.create_task(
            _send_daemon_request(
                socket_path, DaemonRequest(id="req-drain", method="slow")
            )
        )
        await asyncio.wait_for(handler_started.wait(), timeout=0.5)

        stop_task = asyncio.create_task(server.stop())
        await asyncio.sleep(0.01)
        stop_is_waiting_for_drain = not stop_task.done()
        handler_release.set()
        await asyncio.wait_for(stop_task, timeout=0.5)
        response = await asyncio.wait_for(response_task, timeout=0.5)

        return stop_is_waiting_for_drain, response, socket_path.exists()

    stop_is_waiting_for_drain, response, socket_exists = asyncio.run(scenario())

    assert stop_is_waiting_for_drain is True
    assert response.ok is True
    assert response.id == "req-drain"
    assert socket_exists is False


def test_prepare_socket_path_removes_unreachable_stale_socket(tmp_path: Path):
    socket_path = tmp_path / "runtime" / "daemon.sock"
    socket_path.parent.mkdir(mode=0o700)
    stale_socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        stale_socket.bind(str(socket_path))
    finally:
        stale_socket.close()

    lock = prepare_socket_path(socket_path)
    try:
        assert not socket_path.exists()
        assert stat.S_IMODE(socket_path.parent.stat().st_mode) == 0o700
    finally:
        lock.release()


def test_prepare_socket_path_rejects_existing_insecure_runtime_without_chmod(
    tmp_path: Path,
):
    socket_path = tmp_path / "runtime" / "daemon.sock"
    socket_path.parent.mkdir()
    os.chmod(socket_path.parent, 0o755)

    try:
        prepare_socket_path(socket_path)
    except DaemonRuntimePathError as exc:
        assert "must be mode 0700" in exc.message
    else:
        raise AssertionError("expected insecure runtime directory to fail")

    assert stat.S_IMODE(socket_path.parent.stat().st_mode) == 0o755


def test_prepare_socket_path_rejects_relative_socket_parent_without_chmod(
    monkeypatch,
    tmp_path: Path,
):
    project_dir = tmp_path / "project"
    project_dir.mkdir()
    os.chmod(project_dir, 0o755)
    monkeypatch.chdir(project_dir)

    try:
        prepare_socket_path(Path("daemon.sock"))
    except DaemonRuntimePathError as exc:
        assert "must be mode 0700" in exc.message
    else:
        raise AssertionError("expected bare relative socket parent to fail")

    assert stat.S_IMODE(project_dir.stat().st_mode) == 0o755


def test_prepare_socket_path_rejects_symlink_runtime_directory(tmp_path: Path):
    real_runtime = tmp_path / "real-runtime"
    linked_runtime = tmp_path / "linked-runtime"
    real_runtime.mkdir()
    linked_runtime.symlink_to(real_runtime)

    try:
        prepare_socket_path(linked_runtime / "daemon.sock")
    except DaemonRuntimePathError as exc:
        assert "must not be a symlink" in exc.message
    else:
        raise AssertionError("expected symlink runtime directory to fail")


def test_health_does_not_import_heavy_modules(tmp_path: Path):
    heavy_prefixes = (
        "agent_sec_cli.code_scanner",
        "agent_sec_cli.pii_checker",
        "agent_sec_cli.prompt_scanner",
        "agent_sec_cli.security_middleware",
        "agent_sec_cli.skill_ledger",
    )
    before = _matching_modules(heavy_prefixes)

    snapshot = build_health_snapshot(
        DaemonRuntime(socket_path=tmp_path / "daemon.sock")
    )
    registry = create_default_registry()

    assert snapshot["status"] == "ok"
    assert snapshot["prompt_scan"]["status"] == "pending"
    assert registry.methods() == ("daemon.health",)
    assert _matching_modules(heavy_prefixes) == before


def test_completion_log_is_emitted_when_inflight_request_is_cancelled(
    tmp_path: Path, caplog
):
    """Cancellation during drain must still flush a completion log line."""

    caplog.set_level(logging.INFO, logger="agent-sec-core.daemon")

    async def scenario():
        socket_path = tmp_path / "runtime" / "daemon.sock"
        handler_started = asyncio.Event()
        hang_forever = asyncio.Event()

        async def hang_handler(
            _request: DaemonRequest, _runtime: DaemonRuntime
        ) -> HandlerResult:
            handler_started.set()
            await hang_forever.wait()
            return HandlerResult(data={})

        registry = MethodRegistry()
        registry.register(
            MethodSpec(
                method="hang",
                handler=hang_handler,
                lifecycle="test",
                timeout_ms=60_000,
            )
        )
        server = DaemonServer(socket_path=socket_path, registry=registry)
        # Force drain to cancel pending tasks immediately instead of waiting.
        server._drain_timeout_seconds = 0.0
        await server.start()

        client_task = asyncio.create_task(
            _send_daemon_request(
                socket_path,
                DaemonRequest(id="req-cancelled", method="hang", timeout_ms=60_000),
            )
        )
        await asyncio.wait_for(handler_started.wait(), timeout=0.5)

        await server.stop()

        with contextlib.suppress(Exception):
            await client_task

    asyncio.run(scenario())

    matching_logs = [
        json.loads(record.message)
        for record in caplog.records
        if record.name == "agent-sec-core.daemon"
        and _is_json(record.message)
        and json.loads(record.message).get("request_id") == "req-cancelled"
    ]

    assert matching_logs, "expected completion log for cancelled in-flight request"
    payload = matching_logs[-1]
    assert payload["event"] == "daemon_request_completed"
    assert payload["method"] == "hang"
    assert payload["ok"] is False


def _is_json(text: str) -> bool:
    try:
        json.loads(text)
    except (json.JSONDecodeError, TypeError):
        return False
    return True


def test_completion_log_outputs_structured_fields(caplog):
    caplog.set_level(logging.INFO, logger="agent-sec-core.daemon")

    _log_request_completion(
        request_id="req-log",
        method="daemon.health",
        response=success_response("req-log"),
        started=time.monotonic() - 0.01,
        bytes_in=12,
        bytes_out=34,
    )

    payload = json.loads(caplog.records[-1].message)
    assert payload["event"] == "daemon_request_completed"
    assert payload["request_id"] == "req-log"
    assert payload["method"] == "daemon.health"
    assert payload["ok"] is True
    assert payload["exit_code"] == 0
    assert payload["error_code"] is None
    assert payload["bytes_in"] == 12
    assert payload["bytes_out"] == 34
    assert "latency_ms" in payload


async def _send_daemon_request(
    socket_path: Path,
    request: DaemonRequest,
) -> DaemonResponse:
    reader, writer = await asyncio.open_unix_connection(str(socket_path))
    writer.write(serialize_request(request))
    await writer.drain()
    line = await reader.readline()
    writer.close()
    await writer.wait_closed()
    return parse_response_line(line)


def _matching_modules(prefixes: tuple[str, ...]) -> set[str]:
    return {
        module_name
        for module_name in sys.modules
        if any(
            module_name == prefix or module_name.startswith(f"{prefix}.")
            for prefix in prefixes
        )
    }
