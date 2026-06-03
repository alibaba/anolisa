"""Tests for daemon protocol parsing and dispatch."""

import asyncio
from pathlib import Path

import pytest
from agent_sec_cli.daemon.errors import BadRequestError, PayloadTooLargeError
from agent_sec_cli.daemon.protocol import (
    MAX_TIMEOUT_MS,
    DaemonRequest,
    NDJSONFrameParser,
    parse_request_line,
)
from agent_sec_cli.daemon.registry import (
    HandlerResult,
    MethodRegistry,
    MethodSpec,
    dispatch_request,
)
from agent_sec_cli.daemon.runtime import DaemonRuntime


def test_parse_request_rejects_malformed_json():
    with pytest.raises(BadRequestError, match="valid JSON"):
        parse_request_line(b"{bad-json}\n")


def test_parse_request_rejects_non_object_request():
    with pytest.raises(BadRequestError, match="JSON object"):
        parse_request_line(b'["daemon.health"]\n')


def test_frame_parser_handles_partial_and_coalesced_lines():
    parser = NDJSONFrameParser(max_frame_bytes=1024)

    assert parser.feed(b'{"id":"req-1"') == []
    frames = parser.feed(
        b',"method":"daemon.health"}\n{"id":"req-2","method":"daemon.health"}\n'
    )

    assert [parse_request_line(frame).id for frame in frames] == ["req-1", "req-2"]


def test_frame_parser_rejects_oversized_payload():
    parser = NDJSONFrameParser(max_frame_bytes=10)

    with pytest.raises(PayloadTooLargeError):
        parser.feed(b"a" * 11)


def test_parse_request_generates_missing_request_id():
    request = parse_request_line(b'{"method":"daemon.health"}\n')

    assert request.id.startswith("daemon-")
    assert request.method == "daemon.health"
    assert request.params == {}
    assert request.trace_context == {}


def test_parse_request_accepts_timeout_ms_at_max():
    request = parse_request_line(
        f'{{"method":"daemon.health","timeout_ms":{MAX_TIMEOUT_MS}}}\n'.encode()
    )

    assert request.timeout_ms == MAX_TIMEOUT_MS


def test_parse_request_rejects_timeout_ms_above_max():
    over_max = MAX_TIMEOUT_MS + 1
    with pytest.raises(BadRequestError, match="must not exceed"):
        parse_request_line(
            f'{{"method":"daemon.health","timeout_ms":{over_max}}}\n'.encode()
        )


def test_dispatch_rejects_unknown_method(tmp_path: Path):
    async def scenario():
        request = DaemonRequest(id="req-unknown", method="unknown.method")
        response = await dispatch_request(
            request,
            MethodRegistry(),
            DaemonRuntime(socket_path=tmp_path / "daemon.sock"),
        )
        return response

    response = asyncio.run(scenario())

    assert response.id == "req-unknown"
    assert response.ok is False
    assert response.error == {
        "code": "unknown_method",
        "message": "unknown daemon method: unknown.method",
    }


def test_dispatch_applies_request_timeout(tmp_path: Path):
    async def slow_handler(
        _request: DaemonRequest, _runtime: DaemonRuntime
    ) -> HandlerResult:
        await asyncio.sleep(0.05)
        return HandlerResult(data={"done": True})

    async def scenario():
        registry = MethodRegistry()
        registry.register(
            MethodSpec(
                method="slow",
                handler=slow_handler,
                lifecycle="test",
                timeout_ms=1000,
            )
        )
        request = DaemonRequest(id="req-timeout", method="slow", timeout_ms=1)
        response = await dispatch_request(
            request,
            registry,
            DaemonRuntime(socket_path=tmp_path / "daemon.sock"),
        )
        return response

    response = asyncio.run(scenario())

    assert response.id == "req-timeout"
    assert response.ok is False
    assert response.error == {
        "code": "timeout",
        "message": "daemon request timed out after 1 ms",
    }
