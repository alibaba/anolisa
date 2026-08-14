"""Unit tests for the AgentScope 1.x Tokenless integration."""

from __future__ import annotations

import importlib
import json
import sys
import types
import unittest
from dataclasses import dataclass
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


@dataclass
class _ToolResponse:
    content: list[dict]
    metadata: dict | None = None
    stream: bool = False
    is_last: bool = True
    is_interrupted: bool = False
    id: str = "response-id"


@dataclass
class _RegisteredTool:
    name: str
    original_func: object
    postprocess_func: object | None = None


class _Toolkit:
    def __init__(self) -> None:
        self.tools: dict[str, _RegisteredTool] = {}

    def add(self, name: str, postprocess_func: object | None = None) -> None:
        self.tools[name] = _RegisteredTool(name, object(), postprocess_func)

    def register_tool_function(
        self,
        tool_func: object,
        *,
        json_schema: dict,
    ) -> None:
        name = json_schema["function"]["name"]
        self.tools[name] = _RegisteredTool(name, tool_func)


class _Message:
    def __init__(self, content: object, metadata: object | None = None) -> None:
        self.content = content
        self.metadata = metadata


class _Memory:
    def __init__(self) -> None:
        self.messages: list[_Message] = []

    async def get_memory(self) -> list[_Message]:
        return self.messages


@dataclass
class _Agent:
    name: str
    toolkit: _Toolkit
    memory: _Memory


def _install_dependency_stubs() -> None:
    native = types.ModuleType("anolisa_tokenless._native")
    native.CompressionResult = _CompressionResult
    native.TokenlessError = _TokenlessError
    native.TokenlessRuntime = _TokenlessRuntime
    native.__version__ = "0.0.0-test"

    agentscope = types.ModuleType("agentscope")
    agentscope.__path__ = []
    agentscope.__version__ = "1.0.11"

    message = types.ModuleType("agentscope.message")
    message.TextBlock = lambda **kwargs: kwargs

    tool = types.ModuleType("agentscope.tool")
    tool.ToolResponse = _ToolResponse

    sys.modules.update(
        {
            "anolisa_tokenless._native": native,
            "agentscope": agentscope,
            "agentscope.message": message,
            "agentscope.tool": tool,
        },
    )


_install_dependency_stubs()
sys.path.insert(0, str(_RUNTIME_SRC))
sys.path.insert(0, str(_PACKAGE_SRC))

integration = importlib.import_module("tokenless_agentscope")
TokenlessAgentScope = integration.TokenlessAgentScope
TokenlessConfig = integration.TokenlessConfig


class AgentScopeV1Test(unittest.IsolatedAsyncioTestCase):
    def setUp(self) -> None:
        toolkit = _Toolkit()
        toolkit.add("large_result")
        self.agent = _Agent("agent-1", toolkit, _Memory())
        self.integration = TokenlessAgentScope(TokenlessConfig(min_chars=0))

    async def test_install_compresses_and_preserves_response_fields(self) -> None:
        self.integration.install(self.agent)
        original = _ToolResponse(
            content=[
                {"type": "text", "text": '{"payload":"noise","answer":42}'},
                {"type": "image", "source": "unchanged"},
            ],
            metadata={"source": "tool"},
            id="result-1",
        )

        def compress(input_text: str, **kwargs: object) -> _CompressionResult:
            self.assertEqual(json.loads(input_text)["answer"], 42)
            self.assertEqual(kwargs["agent_id"], "agent-1")
            self.assertEqual(kwargs["tool_use_id"], "call-1")
            return _CompressionResult('{"answer":42}', True)

        self.integration._compressor.runtime.compress_impl = compress
        postprocess = self.agent.toolkit.tools["large_result"].postprocess_func
        result = await postprocess(
            {"name": "large_result", "id": "call-1"},
            original,
        )

        self.assertIsNot(result, original)
        self.assertEqual(result.content[0]["text"], '{"answer":42}')
        self.assertIs(result.content[1], original.content[1])
        self.assertIs(result.metadata, original.metadata)
        self.assertEqual(result.id, "result-1")

    async def test_existing_postprocessor_runs_before_tokenless(self) -> None:
        async def previous(_tool_call: dict, response: _ToolResponse) -> _ToolResponse:
            return _ToolResponse(
                content=[{"type": "text", "text": response.content[0]["text"] * 2}]
            )

        self.agent.toolkit.tools["large_result"].postprocess_func = previous
        self.integration.install(self.agent)
        observed: list[str] = []

        def compress(input_text: str, **_kwargs: object) -> _CompressionResult:
            observed.append(json.loads(input_text))
            return _CompressionResult(json.dumps("short"), True)

        self.integration._compressor.runtime.compress_impl = compress
        postprocess = self.agent.toolkit.tools["large_result"].postprocess_func
        result = await postprocess(
            {"name": "large_result", "id": "call"},
            _ToolResponse(content=[{"type": "text", "text": "long text " * 20}]),
        )
        self.assertEqual(observed, ["long text " * 40])
        self.assertEqual(result.content[0]["text"], "short")

    async def test_skips_partial_interrupted_and_error_responses(self) -> None:
        self.integration.install(self.agent)

        def should_not_run(*_args: object, **_kwargs: object) -> _CompressionResult:
            raise AssertionError("compression should have been skipped")

        self.integration._compressor.runtime.compress_impl = should_not_run
        postprocess = self.agent.toolkit.tools["large_result"].postprocess_func
        cases = [
            _ToolResponse(content=[{"type": "text", "text": "x" * 500}], is_last=False),
            _ToolResponse(
                content=[{"type": "text", "text": "x" * 500}], is_interrupted=True
            ),
            _ToolResponse(content=[{"type": "text", "text": "Error: failed"}]),
            _ToolResponse(
                content=[{"type": "text", "text": "x" * 500}],
                metadata={"success": False},
            ),
        ]
        for response in cases:
            result = await postprocess(
                {"name": "large_result", "id": "call"},
                response,
            )
            self.assertIs(result, response)

    async def test_retrieval_is_marker_scoped_and_byte_exact(self) -> None:
        stash_hash = "0123456789abcdef01234567"
        self.integration.install(self.agent)
        retrieve = self.agent.toolkit.tools["tokenless_retrieve"].original_func
        self.integration._compressor.runtime.retrieve_impl = lambda value: (
            "你好\n" if value == stash_hash else "wrong"
        )

        missing = await retrieve(hash=stash_hash)
        self.assertTrue(missing.content[0]["text"].startswith("Error:"))

        self.agent.memory.messages.append(
            _Message("not visible", {"marker": f"<<tokenless:{stash_hash}>>"}),
        )
        metadata_only = await retrieve(hash=stash_hash)
        self.assertTrue(metadata_only.content[0]["text"].startswith("Error:"))

        self.agent.memory.messages.append(
            _Message(f"compressed <<tokenless:{stash_hash}>>"),
        )
        recovered = await retrieve(hash=stash_hash.upper())
        self.assertEqual(recovered.content[0]["text"], "你好\n")

    async def test_install_is_idempotent_and_rejects_collision(self) -> None:
        self.integration.install(self.agent)
        first = self.agent.toolkit.tools["large_result"].postprocess_func
        self.integration.install(self.agent)
        self.assertIs(self.agent.toolkit.tools["large_result"].postprocess_func, first)

        toolkit = _Toolkit()
        toolkit.add("tokenless_retrieve")
        agent = _Agent("agent-2", toolkit, _Memory())
        other = TokenlessAgentScope()
        with self.assertRaisesRegex(ValueError, "already contains"):
            other.install(agent)


if __name__ == "__main__":
    unittest.main()
