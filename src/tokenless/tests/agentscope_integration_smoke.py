#!/usr/bin/env python3
"""Exercise the installed integration with AgentScope and native Tokenless."""

from __future__ import annotations

import asyncio
import json
import re
import tempfile
from pathlib import Path
from typing import ClassVar

import agentscope
from agentscope.agent import Agent
from agentscope.message import TextBlock, ToolCallBlock
from agentscope.permission import PermissionBehavior, PermissionDecision
from agentscope.tool import ToolBase, ToolChunk, Toolkit
from tokenless_agentscope import TokenlessMiddleware

_RECOVERY_PAYLOAD = "RECOVERY_SENTINEL=世界\n" + ("内容" * 3_000) + "TRAILING_NEWLINE\n"


class LargeResultTool(ToolBase):
    """Return enough structured output to exercise reversible compression."""

    name = "large_result"
    description = "Return a large deterministic JSON result."
    input_schema: ClassVar[dict] = {
        "type": "object",
        "properties": {},
        "additionalProperties": False,
    }
    is_concurrency_safe = True
    is_read_only = True

    async def check_permissions(
        self,
        tool_input: dict,
        context: object,
    ) -> PermissionDecision:
        del tool_input, context
        return PermissionDecision(
            behavior=PermissionBehavior.ALLOW,
            message="The fixture is read-only.",
        )

    async def call(self) -> ToolChunk:
        payload = {
            "answer": "ORCHID-7291",
            "payload": _RECOVERY_PAYLOAD,
        }
        return ToolChunk(content=[TextBlock(text=json.dumps(payload, ensure_ascii=False))])


class ExistingRetrieveTool(ToolBase):
    """Represent an application tool that already uses the default name."""

    name = "tokenless_retrieve"
    description = "Existing application retrieval tool."
    input_schema: ClassVar[dict] = {
        "type": "object",
        "properties": {},
        "additionalProperties": False,
    }
    is_concurrency_safe = True
    is_read_only = True

    async def check_permissions(
        self,
        tool_input: dict,
        context: object,
    ) -> PermissionDecision:
        del tool_input, context
        return PermissionDecision(
            behavior=PermissionBehavior.ALLOW,
            message="The fixture is read-only.",
        )

    async def call(self) -> ToolChunk:
        return ToolChunk(content=[TextBlock(text="existing")])


async def main() -> None:
    """Run one real AgentScope middleware and retrieval cycle."""
    version = tuple(int(part) for part in agentscope.__version__.split(".")[:3])
    assert (2, 0, 5) <= version < (2, 1, 0)
    with tempfile.TemporaryDirectory(prefix="tokenless-agentscope-smoke-") as directory:
        existing_retrieve = ExistingRetrieveTool()
        middleware = TokenlessMiddleware(
            mode="aggressive",
            data_dir=Path(directory),
            min_chars=0,
            retrieve_tool_name="tenant_tokenless_retrieve",
        )
        middleware_tool = (await middleware.list_tools())[0]
        app_toolkit = Toolkit(tools=[existing_retrieve, middleware_tool])
        assert await app_toolkit.get_tool("tokenless_retrieve") is existing_retrieve
        assert await app_toolkit.get_tool("tenant_tokenless_retrieve") is middleware_tool

        toolkit = Toolkit(tools=[LargeResultTool(), existing_retrieve])
        await middleware.register_tools(toolkit)
        await middleware.register_tools(toolkit)
        assert await toolkit.get_tool("tokenless_retrieve") is existing_retrieve
        assert await toolkit.get_tool("tenant_tokenless_retrieve") is middleware_tool

        agent = Agent(
            name="smoke",
            system_prompt="Exercise one deterministic tool.",
            model=object(),
            toolkit=toolkit,
            middlewares=[middleware],
        )
        tool_call = ToolCallBlock(id="call-large", name="large_result", input="{}")
        events = [event async for event in agent._acting(tool_call)]
        assert len(events) == 2
        streamed, response = events
        assert "TRAILING_NEWLINE" in streamed.content[0].text
        assert "TRAILING_NEWLINE" not in response.content[0].text
        marker = re.search(r"<<tokenless:([0-9a-f]{24})>>", response.content[0].text)
        assert marker is not None
        assert response.id == "call-large"

        agent.state.summary = response.content
        retrieve_call = ToolCallBlock(
            id="call-retrieve",
            name="tenant_tokenless_retrieve",
            input=json.dumps({"hash": marker.group(1).upper()}),
        )
        retrieved = [event async for event in toolkit.call_tool(retrieve_call, agent.state)]
        assert len(retrieved) == 2
        assert retrieved[0].content[0].text == _RECOVERY_PAYLOAD


if __name__ == "__main__":
    asyncio.run(main())
