#!/usr/bin/env python3
"""Unit tests for the AgentScope 1.x full-lifecycle integration."""

from __future__ import annotations

import importlib
import json
import sys
import types
import unittest
from dataclasses import dataclass, replace
from pathlib import Path

_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(_ROOT / "python" / "tokenless" / "python"))
sys.path.insert(0, str(_ROOT / "python" / "agentscope" / "src"))


class _TokenlessError(Exception):
    pass


@dataclass
class _CompressionResult:
    output: str
    applied: bool


class _Runtime:
    def __init__(self, data_dir=None, **_kwargs):
        self.data_dir = str(data_dir or "/tmp/tokenless-test")
        self.compress_impl = None
        self.retrieve_impl = None

    def compress_response(self, value: str, **kwargs):
        if self.compress_impl:
            return self.compress_impl(value, **kwargs)
        return _CompressionResult(value, False)

    def compress_schema(self, value: str, **_kwargs):
        return _CompressionResult(value, False)

    def compress_toon(self, value: str, **_kwargs):
        return _CompressionResult(value, False)

    def retrieve(self, value: str) -> str:
        if self.retrieve_impl:
            return self.retrieve_impl(value)
        raise _TokenlessError("missing")


@dataclass
class _Response:
    content: list[dict]
    metadata: dict | None = None
    is_last: bool = True
    is_interrupted: bool = False


@dataclass
class _Registered:
    original_func: object
    json_schema: dict
    postprocess_func: object | None = None


class _Toolkit:
    def __init__(self) -> None:
        self.tools = {}

    def register_tool_function(self, func, *args, **kwargs) -> None:
        del args
        schema = kwargs.get("json_schema") or {
            "type": "function",
            "function": {"name": func.__name__, "parameters": {}},
        }
        self.tools[schema["function"]["name"]] = _Registered(
            func, schema, kwargs.get("postprocess_func")
        )


def _install_stubs() -> None:
    native = types.ModuleType("anolisa_tokenless._native")
    native._StatsQuery = object
    native.CompressionResult = _CompressionResult
    native.TokenlessError = _TokenlessError
    native.TokenlessRuntime = _Runtime
    native.__version__ = "0.0.0-test"
    agentscope = types.ModuleType("agentscope")
    agentscope.__path__ = []
    agentscope.__version__ = "1.0.11"
    message = types.ModuleType("agentscope.message")
    message.TextBlock = lambda **kwargs: kwargs
    tool = types.ModuleType("agentscope.tool")
    tool.Toolkit = _Toolkit
    tool.ToolResponse = _Response
    sys.modules.update(
        {
            "anolisa_tokenless._native": native,
            "agentscope": agentscope,
            "agentscope.message": message,
            "agentscope.tool": tool,
        }
    )


_install_stubs()
api = importlib.import_module("tokenless_agentscope")


class _Model:
    stream = False

    async def __call__(self, *args, **kwargs):
        return args, kwargs


class _Agent:
    def __init__(self, toolkit) -> None:
        self.name = "agent-1"
        self.toolkit = toolkit
        self.model = _Model()
        self.hooks = {}

    def register_instance_hook(self, kind, name, hook) -> None:
        self.hooks[(kind, name)] = hook


class AgentScopeV1Test(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        self.integration = api.TokenlessAgentScope(
            api.TokenlessConfig(min_chars=0, rtk_enabled=False)
        )
        self.toolkit = self.integration.create_toolkit()

        async def large_result():
            return _Response([])

        self.toolkit.register_tool_function(large_result)
        self.agent = _Agent(self.toolkit)
        self.integration.install(self.agent, session_id="session-1")

    async def test_dynamic_registration_is_wrapped_and_attributed(self) -> None:
        async def dynamic():
            return _Response([])

        self.toolkit.register_tool_function(dynamic)
        registered = self.toolkit.tools["dynamic"]

        def compress(value: str, **kwargs) -> _CompressionResult:
            self.assertEqual(kwargs["agent_id"], "agent-1")
            self.assertEqual(kwargs["session_id"], "session-1")
            self.assertEqual(kwargs["tool_use_id"], "call-1")
            return _CompressionResult(json.dumps("short"), True)

        self.integration.sdk.runtime.compress_impl = compress
        response = await registered.postprocess_func(
            {"name": "dynamic", "id": "call-1", "input": {}},
            _Response([{"type": "text", "text": "long text " * 100}]),
        )
        self.assertEqual(response.content[0]["text"], "short")

    async def test_model_proxy_compresses_tools_and_tracks_markers(self) -> None:
        marker = "0123456789abcdef01234567"

        async def before_model(request):
            return type(request)(
                request.tools,
                request.visible_context,
                request.attribution,
                frozenset({marker}),
            )

        self.integration.sdk.before_model = before_model
        await self.agent.model(
            [],
            tools=self.toolkit.get_json_schemas()
            if hasattr(self.toolkit, "get_json_schemas")
            else [],
        )
        self.assertEqual(self.toolkit.visible_markers, frozenset({marker}))

    async def test_pre_acting_preserves_original_call(self) -> None:
        original = {"name": "api", "id": "call-2", "input": {"value": 1}}
        hook = self.agent.hooks[("pre_acting", "tokenless")]
        result = await hook(self.agent, {"tool_call": original})
        self.assertIsNot(result["tool_call"], original)
        self.assertEqual(original["input"], {"value": 1})

    async def test_rtk_state_survives_until_the_final_response(self) -> None:
        async def rewrite(call):
            return replace(
                call,
                arguments={**call.arguments, "command": "rtk grep needle file.txt"},
                rewritten=True,
            )

        self.integration.sdk.before_tool_call = rewrite
        original = {
            "name": "shell",
            "id": "call-rtk",
            "input": {"command": "grep needle file.txt"},
        }
        hook = self.agent.hooks[("pre_acting", "tokenless")]
        transformed = (await hook(self.agent, {"tool_call": original}))["tool_call"]

        self.assertEqual(original["input"]["command"], "grep needle file.txt")
        self.assertEqual(transformed["input"]["command"], "rtk grep needle file.txt")
        self.assertIn("call-rtk", self.toolkit.rewritten_calls)

        def fail_compression(_value: str, **_kwargs) -> _CompressionResult:
            raise AssertionError("RTK output reached response compression")

        self.integration.sdk.runtime.compress_impl = fail_compression
        partial = _Response(
            [{"type": "text", "text": "stream"}],
            is_last=False,
        )
        self.assertIs(
            await self.integration._after_tool(self.toolkit, transformed, partial),
            partial,
        )
        self.assertIn("call-rtk", self.toolkit.rewritten_calls)

        final = _Response(
            [
                {
                    "type": "text",
                    "text": json.dumps({"items": list(range(100))}),
                }
            ]
        )
        self.assertIs(
            await self.integration._after_tool(self.toolkit, transformed, final),
            final,
        )
        self.assertNotIn("call-rtk", self.toolkit.rewritten_calls)

    def test_retrieve_name_collision_is_rejected(self) -> None:
        original = self.toolkit.tools["tokenless_retrieve"].original_func

        async def tokenless_retrieve():
            return _Response([])

        with self.assertRaisesRegex(ValueError, "reserved"):
            self.toolkit.register_tool_function(tokenless_retrieve)
        with self.assertRaisesRegex(ValueError, "reserved"):
            self.toolkit.register_tool_function(
                lambda: _Response([]),
                func_name="tokenless_retrieve",
                namesake_strategy="override",
            )
        self.assertIs(
            self.toolkit.tools["tokenless_retrieve"].original_func,
            original,
        )

    def test_requires_tokenless_toolkit_and_explicit_session(self) -> None:
        other = api.TokenlessAgentScope(api.TokenlessConfig(rtk_enabled=False))
        with self.assertRaisesRegex(RuntimeError, "installed before use"):
            other._attribution()
        with self.assertRaisesRegex(ValueError, "session_id"):
            other.install(_Agent(other.create_toolkit()), session_id="")
        with self.assertRaisesRegex(TypeError, "create_toolkit"):
            other.install(_Agent(_Toolkit()), session_id="session")


if __name__ == "__main__":
    unittest.main()
