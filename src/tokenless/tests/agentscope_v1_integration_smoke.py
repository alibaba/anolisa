"""Exercise the installed integration with AgentScope 1.x and Tokenless."""

from __future__ import annotations

import asyncio
import json
import re
import tempfile
from pathlib import Path
from types import SimpleNamespace

import agentscope
from agentscope.memory import InMemoryMemory
from agentscope.message import Msg, TextBlock, ToolResultBlock, ToolUseBlock
from agentscope.tool import Toolkit, ToolResponse
from tokenless_agentscope import TokenlessAgentScope, TokenlessConfig

_RECOVERY_PAYLOAD = "RECOVERY_SENTINEL=世界\n" + ("内容" * 3_000) + "TRAILING_NEWLINE\n"


async def large_result() -> ToolResponse:
    """Return enough structured output to exercise reversible compression."""
    payload = {
        "answer": "ORCHID-7291",
        "payload": _RECOVERY_PAYLOAD,
    }
    return ToolResponse(
        content=[TextBlock(type="text", text=json.dumps(payload, ensure_ascii=False))],
    )


async def main() -> None:
    """Run one real AgentScope 1.x postprocessor and retrieval cycle."""
    version = tuple(int(part) for part in agentscope.__version__.split(".")[:3])
    assert (1, 0, 11) <= version < (1, 1, 0)
    with tempfile.TemporaryDirectory(
        prefix="tokenless-agentscope-v1-smoke-"
    ) as directory:
        toolkit = Toolkit()
        toolkit.register_tool_function(large_result)
        memory = InMemoryMemory()
        agent = SimpleNamespace(name="smoke", toolkit=toolkit, memory=memory)
        integration = TokenlessAgentScope(
            TokenlessConfig(
                mode="aggressive",
                data_dir=Path(directory),
                min_chars=0,
            ),
        )
        integration.install(agent)

        tool_call = ToolUseBlock(
            type="tool_use",
            id="call-large",
            name="large_result",
            input={},
        )
        responses = [
            chunk async for chunk in await toolkit.call_tool_function(tool_call)
        ]
        assert len(responses) == 1
        response = responses[0]
        assert "TRAILING_NEWLINE" not in response.content[0]["text"]
        marker = re.search(r"<<tokenless:([0-9a-f]{24})>>", response.content[0]["text"])
        assert marker is not None

        await memory.add(
            Msg(
                name="system",
                role="system",
                content=[
                    ToolResultBlock(
                        type="tool_result",
                        id="call-large",
                        name="large_result",
                        output=response.content,
                    ),
                ],
            ),
        )
        retrieve_call = ToolUseBlock(
            type="tool_use",
            id="call-retrieve",
            name="tokenless_retrieve",
            input={"hash": marker.group(1).upper()},
        )
        retrieved = [
            chunk async for chunk in await toolkit.call_tool_function(retrieve_call)
        ]
        assert len(retrieved) == 1
        assert retrieved[0].content[0]["text"] == _RECOVERY_PAYLOAD


if __name__ == "__main__":
    asyncio.run(main())
