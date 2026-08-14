#!/usr/bin/env python3
"""Unit tests for the Tokenless AgentScope integration."""

from __future__ import annotations

import asyncio
import copy
import importlib
import json
import sys
import threading
import types
import unittest
from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
_PACKAGE_SRC = _REPO_ROOT / "python" / "agentscope" / "src"
_RUNTIME_SRC = _REPO_ROOT / "python" / "tokenless" / "python"


class _TokenlessError(Exception):
    """Test double for the native package exception."""


@dataclass
class _CompressionResult:
    output: str
    applied: bool


class _TokenlessRuntime:
    """Controllable test double for the native runtime."""

    def __init__(self, data_dir: str | None = None) -> None:
        self.data_dir = data_dir
        self.compress_impl = None
        self.retrieve_impl = None

    def compress_response(
        self, input_text: str, **kwargs: object
    ) -> _CompressionResult:
        if self.compress_impl is None:
            return _CompressionResult(input_text, False)
        return self.compress_impl(input_text, **kwargs)

    def retrieve(self, hash_value: str) -> str:
        if self.retrieve_impl is None:
            raise _TokenlessError(f"no stashed payload for hash: {hash_value}")
        return self.retrieve_impl(hash_value)


def _install_dependency_stubs() -> None:
    """Install the AgentScope and native-runtime API surfaces used here."""
    native = types.ModuleType("anolisa_tokenless._native")
    native.CompressionResult = _CompressionResult
    native.TokenlessError = _TokenlessError
    native.TokenlessRuntime = _TokenlessRuntime
    native.__version__ = "0.0.0-test"

    agentscope = types.ModuleType("agentscope")
    agentscope.__path__ = []
    agentscope.__version__ = "2.0.5"

    message = types.ModuleType("agentscope.message")

    class ToolResultState(StrEnum):
        RUNNING = "running"
        SUCCESS = "success"
        ERROR = "error"
        DENIED = "denied"
        INTERRUPTED = "interrupted"

    @dataclass
    class TextBlock:
        text: str
        id: str = "text-id"

        def model_copy(
            self,
            *,
            update: dict[str, object] | None = None,
            deep: bool = False,
        ) -> TextBlock:
            replacement = copy.deepcopy(self) if deep else copy.copy(self)
            for key, value in (update or {}).items():
                setattr(replacement, key, value)
            return replacement

    message.TextBlock = TextBlock
    message.ToolResultState = ToolResultState

    middleware = types.ModuleType("agentscope.middleware")

    class MiddlewareBase:
        async def list_tools(self) -> list[object]:
            return []

    middleware.MiddlewareBase = MiddlewareBase

    permission = types.ModuleType("agentscope.permission")

    class PermissionBehavior(StrEnum):
        ALLOW = "allow"

    @dataclass
    class PermissionDecision:
        behavior: PermissionBehavior
        message: str

    permission.PermissionBehavior = PermissionBehavior
    permission.PermissionDecision = PermissionDecision

    tool = types.ModuleType("agentscope.tool")

    class ToolBase:
        def __init__(self) -> None:
            pass

        async def call(self, **_kwargs: object):
            raise NotImplementedError

    @dataclass
    class ToolChunk:
        content: list[object]
        state: ToolResultState = ToolResultState.RUNNING
        metadata: dict = field(default_factory=dict)
        id: str = "chunk-id"

    @dataclass
    class ToolResponse:
        content: list[object]
        state: ToolResultState = ToolResultState.SUCCESS
        metadata: dict = field(default_factory=dict)
        id: str = "response-id"

        def model_copy(
            self,
            *,
            update: dict[str, object] | None = None,
            deep: bool = False,
        ) -> ToolResponse:
            replacement = copy.deepcopy(self) if deep else copy.copy(self)
            for key, value in (update or {}).items():
                setattr(replacement, key, value)
            return replacement

    class Toolkit:
        def __init__(self, tools: list[ToolBase] | None = None) -> None:
            self.tools = {item.name: item for item in tools or []}

        async def get_tool(self, name: str):
            return self.tools.get(name)

        async def add_tool(self, item: ToolBase) -> None:
            self.tools[item.name] = item

    tool.ToolBase = ToolBase
    tool.ToolChunk = ToolChunk
    tool.ToolResponse = ToolResponse
    tool.Toolkit = Toolkit

    sys.modules.update(
        {
            "anolisa_tokenless._native": native,
            "agentscope": agentscope,
            "agentscope.message": message,
            "agentscope.middleware": middleware,
            "agentscope.permission": permission,
            "agentscope.tool": tool,
        },
    )


_install_dependency_stubs()
sys.path.insert(0, str(_RUNTIME_SRC))
sys.path.insert(0, str(_PACKAGE_SRC))

integration = importlib.import_module("tokenless_agentscope.middleware")
public_api = importlib.import_module("tokenless_agentscope")
CompressionMode = integration.CompressionMode
TokenlessMiddleware = integration.TokenlessMiddleware
TokenlessAgentScope = public_api.TokenlessAgentScope
TokenlessConfig = public_api.TokenlessConfig
TextBlock = sys.modules["agentscope.message"].TextBlock
ToolResultState = sys.modules["agentscope.message"].ToolResultState
ToolChunk = sys.modules["agentscope.tool"].ToolChunk
ToolResponse = sys.modules["agentscope.tool"].ToolResponse
Toolkit = sys.modules["agentscope.tool"].Toolkit
PermissionBehavior = sys.modules["agentscope.permission"].PermissionBehavior


@dataclass
class _DataBlock:
    data: bytes
    id: str = "data-id"


@dataclass
class _ToolCall:
    name: str
    id: str


class _State:
    def __init__(
        self,
        session_id: str,
        payload: object | None = None,
        *,
        summary: object | None = None,
        middle_context: object | None = None,
    ) -> None:
        self.session_id = session_id
        self.context = payload
        self.summary = summary
        self.middle_context = middle_context

    def model_dump_json(self, *, include: set[str]) -> str:
        return json.dumps(
            {key: getattr(self, key) for key in include},
            ensure_ascii=False,
        )


@dataclass
class _Agent:
    state: _State


async def _collect(generator) -> list[object]:
    return [item async for item in generator]


class MiddlewareTest(unittest.IsolatedAsyncioTestCase):
    async def test_streams_chunks_and_only_replaces_final_response(self) -> None:
        middleware = TokenlessMiddleware(min_chars=0)
        chunk = ToolChunk(content=[TextBlock("streamed")], id="chunk-1")
        data = _DataBlock(b"image", id="image-1")
        response = ToolResponse(
            content=[TextBlock('{"debug":"noise","answer":42}', id="text-1"), data],
            metadata={"source": "tool"},
            id="result-1",
        )

        def compress(input_text: str, **_kwargs: object) -> _CompressionResult:
            self.assertEqual(json.loads(input_text), {"debug": "noise", "answer": 42})
            return _CompressionResult('{"answer":42}', True)

        middleware._runtime.compress_impl = compress

        async def next_handler(**_kwargs):
            yield chunk
            yield response

        output = await _collect(
            middleware.on_acting(
                _Agent(_State("session-1")),
                {"tool_call": _ToolCall("ApiCall", "call-1")},
                next_handler,
            ),
        )

        self.assertIs(output[0], chunk)
        self.assertIsNot(output[1], response)
        self.assertEqual(output[1].content[0].text, '{"answer":42}')
        self.assertEqual(output[1].content[0].id, "text-1")
        self.assertIs(output[1].content[1], data)
        self.assertIs(output[1].metadata, response.metadata)
        self.assertEqual(output[1].id, "result-1")
        self.assertEqual(response.content[0].text, '{"debug":"noise","answer":42}')

    async def test_data_only_response_is_not_copied(self) -> None:
        middleware = TokenlessMiddleware(min_chars=0)
        data = _DataBlock(b"x" * 1024, id="image-1")
        response = ToolResponse(
            content=[data],
            metadata={"nested": ["value"]},
        )

        result = await middleware._compress_response(
            response,
            tool_name="ApiCall",
            session_id="session",
            tool_use_id="call",
        )

        self.assertIs(result, response)
        self.assertIs(result.content[0], data)
        self.assertIs(result.metadata, response.metadata)

    async def test_plain_text_uses_shell_thresholds_and_attribution(self) -> None:
        middleware = TokenlessMiddleware(min_chars=0)
        calls: list[tuple[str, dict[str, object]]] = []

        def compress(input_text: str, **kwargs: object) -> _CompressionResult:
            calls.append((input_text, kwargs))
            return _CompressionResult(
                json.dumps("short <<tokenless:0123456789abcdef01234567>>"),
                True,
            )

        middleware._runtime.compress_impl = compress
        compressed = await middleware._compress_text(
            "x" * 100,
            tool_name="Bash",
            session_id="session-2",
            tool_use_id="call-2",
        )

        self.assertEqual(compressed, "short <<tokenless:0123456789abcdef01234567>>")
        self.assertEqual(json.loads(calls[0][0]), "x" * 100)
        self.assertEqual(calls[0][1]["truncate_strings_at"], 65_536)
        self.assertEqual(calls[0][1]["truncate_arrays_at"], 128)
        self.assertEqual(calls[0][1]["max_depth"], 8)
        self.assertEqual(calls[0][1]["agent_id"], "agentscope")
        self.assertEqual(calls[0][1]["session_id"], "session-2")
        self.assertEqual(calls[0][1]["tool_use_id"], "call-2")
        self.assertIs(calls[0][1]["require_reversible"], True)

    async def test_json_array_keeps_its_type(self) -> None:
        middleware = TokenlessMiddleware(min_chars=0)

        def compress(input_text: str, **_kwargs: object) -> _CompressionResult:
            self.assertEqual(json.loads(input_text), ["noise", "answer"])
            return _CompressionResult('["answer"]', True)

        middleware._runtime.compress_impl = compress
        compressed = await middleware._compress_text(
            '["noise","answer"]',
            tool_name="ApiCall",
            session_id="session",
            tool_use_id="call",
        )
        self.assertEqual(compressed, '["answer"]')

    async def test_modes_select_expected_thresholds(self) -> None:
        conservative = TokenlessMiddleware(mode="conservative")
        balanced = TokenlessMiddleware(mode=CompressionMode.BALANCED)
        aggressive = TokenlessMiddleware(mode="aggressive")
        self.assertEqual(conservative._thresholds_for("Bash"), (1_048_576, 65_536, 32))
        self.assertEqual(balanced._thresholds_for("Bash"), (65_536, 128, 8))
        self.assertEqual(balanced._thresholds_for("ApiCall"), (1_048_576, 65_536, 32))
        self.assertEqual(aggressive._thresholds_for("ApiCall"), (4_096, 32, 8))
        self.assertFalse(conservative._is_excluded("Read"))
        self.assertTrue(balanced._is_excluded("Read"))
        self.assertTrue(aggressive._is_excluded("Read"))

    async def test_skips_excluded_tools_and_unsuccessful_responses(self) -> None:
        middleware = TokenlessMiddleware(min_chars=0, excluded_tools={"CustomRead"})

        def should_not_run(*_args: object, **_kwargs: object) -> _CompressionResult:
            raise AssertionError("compression should have been skipped")

        middleware._runtime.compress_impl = should_not_run
        cases = [
            ("Read", ToolResultState.SUCCESS),
            ("CustomRead", ToolResultState.SUCCESS),
            ("tokenless_retrieve", ToolResultState.SUCCESS),
            ("ApiCall", ToolResultState.ERROR),
            ("ApiCall", ToolResultState.DENIED),
            ("ApiCall", ToolResultState.INTERRUPTED),
        ]
        for tool_name, state in cases:
            response = ToolResponse(content=[TextBlock("x" * 500)], state=state)

            async def next_handler(*, _response=response, **_kwargs):
                yield _response

            output = await _collect(
                middleware.on_acting(
                    _Agent(_State("session")),
                    {"tool_call": _ToolCall(tool_name, "call")},
                    next_handler,
                ),
            )
            self.assertIs(output[0], response)

    async def test_runtime_and_output_failures_preserve_original(self) -> None:
        middleware = TokenlessMiddleware(min_chars=0)
        original = '{"value":"' + "x" * 100 + '"}'
        cases = [
            _TokenlessError("stash unavailable"),
            _CompressionResult(original, False),
            _CompressionResult("not-json", True),
            _CompressionResult("9" * 4_301, True),
            _CompressionResult("[" * 10_000 + "]" * 10_000, True),
            _CompressionResult("[]", True),
            _CompressionResult(original, True),
            _CompressionResult('{"value":"' + "x" * 120 + '"}', True),
        ]
        for result in cases:

            def compress(*_args: object, _result=result, **_kwargs: object):
                if isinstance(_result, Exception):
                    raise _result
                return _result

            middleware._runtime.compress_impl = compress
            self.assertIsNone(
                await middleware._compress_text(
                    original,
                    tool_name="ApiCall",
                    session_id="session",
                    tool_use_id="call",
                ),
            )

        middleware._runtime.compress_impl = (
            lambda *_args, **_kwargs: _CompressionResult("{}", True)
        )
        self.assertIsNone(
            await middleware._compress_text(
                "plain text that should remain text",
                tool_name="ApiCall",
                session_id="session",
                tool_use_id="call",
            ),
        )

    async def test_data_dir_is_owned_by_runtime(self) -> None:
        data_dir = "/tmp/tokenless-agentscope-tenant"
        middleware = TokenlessMiddleware(data_dir=data_dir)
        self.assertEqual(middleware.data_dir, data_dir)
        self.assertEqual(middleware._runtime.data_dir, data_dir)

        with self.assertRaisesRegex(ValueError, "absolute path"):
            TokenlessMiddleware(data_dir="tenant-data")

    async def test_parallel_calls_keep_attribution_separate(self) -> None:
        middleware = TokenlessMiddleware(min_chars=0)
        observed: list[tuple[str, str]] = []
        calling_thread = threading.get_ident()
        worker_threads: set[int] = set()

        def compress(_input_text: str, **kwargs: object) -> _CompressionResult:
            worker_threads.add(threading.get_ident())
            observed.append((str(kwargs["session_id"]), str(kwargs["tool_use_id"])))
            return _CompressionResult('{"ok":true}', True)

        middleware._runtime.compress_impl = compress
        await asyncio.gather(
            middleware._compress_text(
                '{"debug":"one","ok":true}',
                tool_name="ApiCall",
                session_id="session-a",
                tool_use_id="call-a",
            ),
            middleware._compress_text(
                '{"debug":"two","ok":true}',
                tool_name="ApiCall",
                session_id="session-b",
                tool_use_id="call-b",
            ),
        )

        self.assertEqual(
            set(observed),
            {("session-a", "call-a"), ("session-b", "call-b")},
        )
        self.assertNotIn(calling_thread, worker_threads)


class RetrieveToolTest(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self) -> None:
        self.middleware = TokenlessMiddleware(data_dir="/tmp/tokenless-tenant")
        self.tool = (await self.middleware.list_tools())[0]

    async def test_register_tools_is_idempotent_and_rejects_conflict(self) -> None:
        toolkit = Toolkit()
        await self.middleware.register_tools(toolkit)
        await self.middleware.register_tools(toolkit)
        self.assertIs(await toolkit.get_tool("tokenless_retrieve"), self.tool)

        other = type("OtherTool", (), {"name": "tokenless_retrieve"})()
        conflict = Toolkit([other])
        with self.assertRaisesRegex(ValueError, "different 'tokenless_retrieve'"):
            await self.middleware.register_tools(conflict)

    async def test_custom_name_avoids_app_tool_collision(self) -> None:
        existing = type("ExistingTool", (), {"name": "tokenless_retrieve"})()
        middleware = TokenlessMiddleware(
            data_dir="/tmp/tokenless-tenant",
            retrieve_tool_name="tenant_tokenless_retrieve",
        )
        middleware_tool = (await middleware.list_tools())[0]
        app_toolkit = Toolkit([existing, middleware_tool])

        self.assertIs(await app_toolkit.get_tool("tokenless_retrieve"), existing)
        self.assertIs(
            await app_toolkit.get_tool("tenant_tokenless_retrieve"),
            middleware_tool,
        )
        self.assertTrue(middleware._is_excluded("tenant_tokenless_retrieve"))

    async def test_retrieve_tool_name_must_not_be_empty(self) -> None:
        with self.assertRaisesRegex(ValueError, "must not be empty"):
            TokenlessMiddleware(retrieve_tool_name="")

    async def test_retrieve_is_auto_allowed_and_byte_exact(self) -> None:
        stash_hash = "0123456789ABCDEF01234567"
        state = _State(
            "session",
            {"text": "omitted <<tokenless:0123456789abcdef01234567>>"},
        )
        worker_threads: set[int] = set()

        def retrieve(hash_value: str) -> str:
            worker_threads.add(threading.get_ident())
            self.assertEqual(hash_value, stash_hash.lower())
            return "你好\n"

        self.middleware._runtime.retrieve_impl = retrieve
        decision = await self.tool.check_permissions({}, object())
        self.assertEqual(decision.behavior, PermissionBehavior.ALLOW)
        calling_thread = threading.get_ident()
        chunk = await self.tool.call(stash_hash, state)
        self.assertEqual(chunk.state, ToolResultState.RUNNING)
        self.assertEqual(chunk.content[0].text, "你好\n")
        self.assertNotIn(calling_thread, worker_threads)

    async def test_retrieve_rejects_bad_or_unreferenced_hash(self) -> None:
        def should_not_run(_hash_value: str) -> str:
            raise AssertionError("runtime retrieval should not run")

        self.middleware._runtime.retrieve_impl = should_not_run
        bad = await self.tool.call("not-a-hash", _State("session", {}))
        missing = await self.tool.call(
            "0123456789abcdef01234567",
            _State("session", {}),
        )
        self.assertEqual(bad.state, ToolResultState.ERROR)
        self.assertEqual(missing.state, ToolResultState.ERROR)

    async def test_retrieve_accepts_summary_but_not_middle_context(self) -> None:
        stash_hash = "0123456789abcdef01234567"
        self.middleware._runtime.retrieve_impl = lambda _hash: "recovered"
        from_summary = await self.tool.call(
            stash_hash,
            _State("session", summary=f"<<tokenless:{stash_hash}>>"),
        )
        self.assertEqual(from_summary.content[0].text, "recovered")

        rejected = await self.tool.call(
            stash_hash,
            _State(
                "session",
                middle_context={"marker": f"<<tokenless:{stash_hash}>>"},
            ),
        )
        self.assertEqual(rejected.state, ToolResultState.ERROR)

    async def test_retrieve_surfaces_missing_or_expired_payload(self) -> None:
        stash_hash = "0123456789abcdef01234567"
        state = _State("session", f"<<tokenless:{stash_hash}>>")

        def missing(_hash_value: str) -> str:
            raise _TokenlessError(f"no stashed payload for hash: {stash_hash}")

        self.middleware._runtime.retrieve_impl = missing
        chunk = await self.tool.call(stash_hash, state)
        self.assertEqual(chunk.state, ToolResultState.ERROR)
        self.assertIn("no stashed payload", chunk.content[0].text)


class PublicIntegrationTest(unittest.IsolatedAsyncioTestCase):
    async def test_direct_agent_surfaces_share_one_middleware(self) -> None:
        integration = TokenlessAgentScope(
            TokenlessConfig(data_dir="/tmp/tokenless-direct")
        )
        self.assertEqual(integration.middlewares, [integration.middleware])
        self.assertEqual(integration.tools, [integration.middleware.retrieve_tool])

    async def test_app_options_isolate_sessions_without_duplicate_tools(self) -> None:
        integration = TokenlessAgentScope(
            TokenlessConfig(data_dir="/tmp/tokenless-app")
        )
        options = integration.app_options()
        middleware_factory = options["extra_agent_middlewares"]
        tool_factory = options["extra_agent_tools"]

        middlewares = await middleware_factory("user", "agent", "session")
        tools = await tool_factory("user", "agent", "session")
        other = await middleware_factory("user", "agent", "other")

        self.assertEqual(await middlewares[0].list_tools(), [])
        self.assertEqual(middlewares[0].data_dir, tools[0]._compressor.config.data_dir)
        self.assertNotEqual(middlewares[0].data_dir, other[0].data_dir)

    async def test_app_options_requires_explicit_base_directory(self) -> None:
        with self.assertRaisesRegex(ValueError, "data_dir"):
            TokenlessAgentScope().app_options()


class PolicyTest(unittest.TestCase):
    def test_categories_match_canonical_policy(self) -> None:
        policy = json.loads(
            (
                _REPO_ROOT / "adapters/tokenless/common/hooks/tool_categories.json"
            ).read_text(),
        )
        self.assertEqual(
            integration._SKIP_TOOLS, frozenset(policy["layer_1_skip"]["tools"])
        )
        self.assertEqual(
            integration._SHELL_TOOLS, frozenset(policy["layer_2_shell"]["tools"])
        )
        thresholds = policy["layer_2_shell"]["thresholds"]
        self.assertEqual(
            integration._SHELL_THRESHOLDS,
            (
                thresholds["truncate_strings_at"],
                thresholds["truncate_arrays_at"],
                thresholds["max_depth"],
            ),
        )


if __name__ == "__main__":
    unittest.main()
