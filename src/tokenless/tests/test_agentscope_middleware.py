#!/usr/bin/env python3
"""Unit tests for the AgentScope 2.x full-lifecycle middleware."""

from __future__ import annotations

import copy
import importlib
import json
import sys
import types
import unittest
from dataclasses import dataclass, field, replace
from enum import StrEnum
from pathlib import Path

_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(_ROOT / "python" / "tokenless" / "python"))
sys.path.insert(0, str(_ROOT / "python" / "agentscope" / "src"))


class _NativeError(Exception):
    pass


@dataclass
class _CompressionResult:
    output: str
    applied: bool


class _Runtime:
    def __init__(self, data_dir=None, **_kwargs):
        self.data_dir = str(data_dir or "/tmp/tokenless-test")
        self.compress_impl = None
        self.schema_impl = None
        self.toon_impl = None
        self.retrieve_impl = None

    def compress_response(self, value, **kwargs):
        return (
            self.compress_impl(value, **kwargs)
            if self.compress_impl
            else _CompressionResult(value, False)
        )

    def compress_schema(self, value, **kwargs):
        return (
            self.schema_impl(value, **kwargs)
            if self.schema_impl
            else _CompressionResult(value, False)
        )

    def compress_toon(self, value, **_kwargs):
        return (
            self.toon_impl(value)
            if self.toon_impl
            else _CompressionResult(value, False)
        )

    def retrieve(self, value):
        if self.retrieve_impl:
            return self.retrieve_impl(value)
        raise _NativeError("missing")


class _ResultState(StrEnum):
    RUNNING = "running"
    SUCCESS = "success"
    ERROR = "error"
    DENIED = "denied"
    INTERRUPTED = "interrupted"


@dataclass
class _TextBlock:
    text: str

    def model_copy(self, *, update=None, deep=False):
        result = copy.deepcopy(self) if deep else copy.copy(self)
        for key, value in (update or {}).items():
            setattr(result, key, value)
        return result


@dataclass
class _ToolResponse:
    content: list
    state: _ResultState = _ResultState.SUCCESS

    def model_copy(self, *, update=None, deep=False):
        result = copy.deepcopy(self) if deep else copy.copy(self)
        for key, value in (update or {}).items():
            setattr(result, key, value)
        return result


@dataclass
class _ToolChunk:
    content: list
    state: _ResultState = _ResultState.RUNNING


class _ToolBase:
    def __init__(self) -> None:
        pass

    async def call(self, **_kwargs):
        raise NotImplementedError


class _Toolkit:
    def __init__(self, tools=None):
        self.tools = {tool.name: tool for tool in tools or []}

    async def get_tool(self, name):
        return self.tools.get(name)

    async def add_tool(self, tool):
        self.tools[tool.name] = tool


class _MiddlewareBase:
    async def list_tools(self):
        return []


class _PermissionBehavior(StrEnum):
    ALLOW = "allow"


@dataclass
class _PermissionDecision:
    behavior: _PermissionBehavior
    message: str


@dataclass
class _Call:
    id: str
    name: str
    input: str

    def model_copy(self, *, update=None, deep=False):
        result = copy.deepcopy(self) if deep else copy.copy(self)
        for key, value in (update or {}).items():
            setattr(result, key, value)
        return result


@dataclass
class _State:
    session_id: str
    context: list = field(default_factory=list)
    summary: object = ""
    middle_context: dict = field(default_factory=dict)


@dataclass
class _Agent:
    name: str
    state: _State


def _install_stubs() -> None:
    native = types.ModuleType("anolisa_tokenless._native")
    native._StatsQuery = object
    native.CompressionResult = _CompressionResult
    native.TokenlessError = _NativeError
    native.TokenlessRuntime = _Runtime
    native.__version__ = "0.0.0-test"
    agentscope = types.ModuleType("agentscope")
    agentscope.__path__ = []
    agentscope.__version__ = "2.0.5"
    message = types.ModuleType("agentscope.message")
    message.TextBlock = _TextBlock
    message.ToolResultState = _ResultState
    middleware = types.ModuleType("agentscope.middleware")
    middleware.MiddlewareBase = _MiddlewareBase
    permission = types.ModuleType("agentscope.permission")
    permission.PermissionBehavior = _PermissionBehavior
    permission.PermissionDecision = _PermissionDecision
    tool = types.ModuleType("agentscope.tool")
    tool.ToolBase = _ToolBase
    tool.ToolChunk = _ToolChunk
    tool.ToolResponse = _ToolResponse
    tool.Toolkit = _Toolkit
    sys.modules.update(
        {
            "anolisa_tokenless._native": native,
            "agentscope": agentscope,
            "agentscope.message": message,
            "agentscope.middleware": middleware,
            "agentscope.permission": permission,
            "agentscope.tool": tool,
        }
    )


_install_stubs()
api = importlib.import_module("tokenless_agentscope")


async def _collect(generator):
    return [item async for item in generator]


class MiddlewareTest(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        config = api.TokenlessConfig(min_chars=0, rtk_enabled=False)
        self.middleware = api.TokenlessMiddleware(_config=config)
        self.agent = _Agent("agent-2", _State("session-2"))

    async def test_model_call_transforms_tools_and_stores_markers(self) -> None:
        marker = "0123456789abcdef01234567"

        async def before_model(request):
            return type(request)(
                request.tools,
                request.visible_context,
                request.attribution,
                frozenset({marker}),
            )

        self.middleware.sdk.before_model = before_model
        observed = {}

        async def next_handler(**kwargs):
            observed.update(kwargs)
            return "model-response"

        result = await self.middleware.on_model_call(
            self.agent,
            {
                "messages": [],
                "tools": [{"type": "web_search"}],
                "tool_choice": None,
                "current_model": object(),
            },
            next_handler,
        )
        self.assertEqual(result, "model-response")
        self.assertEqual(observed["tools"], [{"type": "web_search"}])
        self.assertEqual(
            self.agent.state.middle_context["anolisa_tokenless"]["visible_markers"],
            [marker],
        )
        self.assertEqual(self.middleware._session_markers, {})

    async def test_acting_preserves_stream_and_transforms_final_text(self) -> None:
        def compress(value, **kwargs):
            self.assertEqual(kwargs["session_id"], "session-2")
            self.assertEqual(kwargs["tool_use_id"], "call-1")
            return _CompressionResult(json.dumps("short"), True)

        self.middleware.sdk.runtime.compress_impl = compress
        chunk = _ToolChunk([_TextBlock("stream")])
        response = _ToolResponse([_TextBlock("long " * 100)])

        async def next_handler(**kwargs):
            self.assertEqual(kwargs["tool_call"].input, "{}")
            yield chunk
            yield response

        output = await _collect(
            self.middleware.on_acting(
                self.agent, {"tool_call": _Call("call-1", "api", "{}")}, next_handler
            )
        )
        self.assertIs(output[0], chunk)
        self.assertEqual(output[1].content[0].text, "short")
        self.assertEqual(response.content[0].text, "long " * 100)

    async def test_rtk_state_reaches_the_final_response(self) -> None:
        async def rewrite(call):
            return replace(
                call,
                arguments={**call.arguments, "command": "rtk grep needle file.txt"},
                rewritten=True,
            )

        self.middleware.sdk.before_tool_call = rewrite

        def fail_compression(_value, **_kwargs):
            raise AssertionError("RTK output reached response compression")

        self.middleware.sdk.runtime.compress_impl = fail_compression
        source = _Call(
            "call-rtk",
            "shell",
            json.dumps({"command": "grep needle file.txt"}),
        )
        chunk = _ToolChunk([_TextBlock("stream")])
        response = _ToolResponse([_TextBlock(json.dumps({"items": list(range(100))}))])

        async def next_handler(**kwargs):
            self.assertEqual(
                json.loads(kwargs["tool_call"].input)["command"],
                "rtk grep needle file.txt",
            )
            yield chunk
            yield response

        output = await _collect(
            self.middleware.on_acting(
                self.agent,
                {"tool_call": source},
                next_handler,
            )
        )

        self.assertEqual(json.loads(source.input)["command"], "grep needle file.txt")
        self.assertIs(output[0], chunk)
        self.assertIs(output[1], response)

    async def test_error_adds_environment_guidance(self) -> None:
        response = _ToolResponse(
            [_TextBlock("command not found")], state=_ResultState.ERROR
        )

        async def next_handler(**_kwargs):
            yield response

        output = await _collect(
            self.middleware.on_acting(
                self.agent, {"tool_call": _Call("call-2", "api", "{}")}, next_handler
            )
        )
        self.assertEqual(output[0].content[0].text, "command not found")
        self.assertIn("Skip retry", output[0].content[1].text)

    async def test_retrieve_uses_only_middleware_marker_state(self) -> None:
        marker = "0123456789abcdef01234567"
        self.agent.state.middle_context["anolisa_tokenless"] = {
            "visible_markers": [marker]
        }
        self.middleware.sdk.runtime.retrieve_impl = lambda value: (
            "payload" if value == marker else "wrong"
        )
        result = await self.middleware.retrieve_tool.call(
            marker.upper(), self.agent.state
        )
        self.assertEqual(result.content[0].text, "payload")

    async def test_retrieve_response_bypasses_all_optimization(self) -> None:
        original = json.dumps({"items": list(range(100))})
        self.middleware.sdk.runtime.toon_impl = lambda _value: _CompressionResult(
            "changed", True
        )
        response = _ToolResponse([_TextBlock(original)])

        async def next_handler(**_kwargs):
            yield response

        output = await _collect(
            self.middleware.on_acting(
                self.agent,
                {
                    "tool_call": _Call(
                        "call-retrieve",
                        "tokenless_retrieve",
                        json.dumps({"hash": "0123456789abcdef01234567"}),
                    )
                },
                next_handler,
            )
        )
        self.assertIs(output[0], response)
        self.assertEqual(output[0].content[0].text, original)

    async def test_register_tools_rejects_collision(self) -> None:
        schema = self.middleware.sdk.retrieve_schema()["function"]
        self.assertEqual(
            self.middleware.retrieve_tool.description, schema["description"]
        )
        self.assertEqual(
            self.middleware.retrieve_tool.input_schema, schema["parameters"]
        )
        toolkit = _Toolkit()
        await self.middleware.register_tools(toolkit)
        self.assertIs(
            await toolkit.get_tool("tokenless_retrieve"), self.middleware.retrieve_tool
        )
        other = api.TokenlessMiddleware(_config=api.TokenlessConfig(rtk_enabled=False))
        with self.assertRaisesRegex(ValueError, "already contains"):
            await other.register_tools(toolkit)


if __name__ == "__main__":
    unittest.main()
