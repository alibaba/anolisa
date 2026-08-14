"""AgentScope 1.x integration for reversible Tokenless compression."""

from __future__ import annotations

import inspect
import json
from dataclasses import replace
from typing import Any

from agentscope.message import TextBlock
from agentscope.tool import ToolResponse
from anolisa_tokenless import (
    RetrievalError,
    TokenlessConfig,
    ToolResponseCompressor,
)

_RETRIEVE_DESCRIPTION = (
    "Recover content omitted at a <<tokenless:HASH>> marker. Call this "
    "only when the omitted content is necessary, passing the marker's "
    "24-character hexadecimal HASH."
)


class TokenlessAgentScope:
    """Stable Tokenless entry point for AgentScope 1.x agents."""

    def __init__(self, config: TokenlessConfig | None = None) -> None:
        """Create an integration that can be bound to one Agent."""
        self.config = config or TokenlessConfig()
        self._compressor = ToolResponseCompressor(self.config)
        self._installed_agent: Any | None = None

    def install(self, agent: Any) -> None:
        """Install postprocessors and marker-scoped retrieval on an Agent."""
        if self._installed_agent is agent:
            return
        if self._installed_agent is not None:
            raise ValueError(
                "A TokenlessAgentScope instance can be installed on only one Agent"
            )
        if not hasattr(agent, "toolkit") or not hasattr(agent, "memory"):
            raise TypeError(
                "AgentScope 1.x integration requires an Agent with toolkit and memory"
            )

        toolkit = agent.toolkit
        if self.config.retrieve_tool_name in toolkit.tools:
            raise ValueError(
                f"Toolkit already contains a different '{self.config.retrieve_tool_name}' tool",
            )

        for registered_tool in toolkit.tools.values():
            registered_tool.postprocess_func = self._wrap_postprocessor(
                agent,
                registered_tool.postprocess_func,
            )

        retrieve = self._build_retrieve_tool(agent)
        toolkit.register_tool_function(
            retrieve,
            json_schema=self._retrieve_schema(),
        )
        self._installed_agent = agent

    def _wrap_postprocessor(self, agent: Any, previous: Any) -> Any:
        async def postprocess(
            tool_call: dict[str, Any], response: ToolResponse
        ) -> ToolResponse:
            if previous is not None:
                processed = previous(tool_call, response)
                if inspect.isawaitable(processed):
                    processed = await processed
                if processed is not None:
                    response = processed
            return await self._compress_response(agent, tool_call, response)

        return postprocess

    async def _compress_response(
        self,
        agent: Any,
        tool_call: dict[str, Any],
        response: ToolResponse,
    ) -> ToolResponse:
        tool_name = tool_call["name"]
        if (
            self._compressor.is_excluded(tool_name)
            or not response.is_last
            or response.is_interrupted
            or (
                response.metadata is not None
                and response.metadata.get("success") is False
            )
            or self._looks_like_error(response)
        ):
            return response

        replacements: dict[int, TextBlock] = {}
        for index, block in enumerate(response.content):
            if block.get("type") != "text":
                continue
            text = block.get("text", "")
            if len(text) < self.config.min_chars:
                continue
            compressed = await self._compressor.compress_text(
                text,
                tool_name=tool_name,
                agent_id=str(agent.name),
                session_id=None,
                tool_use_id=tool_call["id"],
            )
            if compressed is not None:
                replacements[index] = TextBlock(type="text", text=compressed)
        if not replacements:
            return response
        content = [
            replacements.get(index, block)
            for index, block in enumerate(response.content)
        ]
        return replace(response, content=content)

    def _build_retrieve_tool(self, agent: Any) -> Any:
        async def retrieve(hash: str) -> ToolResponse:
            memory = await agent.memory.get_memory()
            visible_context = json.dumps(
                [message.content for message in memory],
                ensure_ascii=False,
                default=str,
            )
            try:
                payload = await self._compressor.retrieve(hash, visible_context)
            except RetrievalError as error:
                return ToolResponse(
                    content=[TextBlock(type="text", text=f"Error: {error}")],
                )
            return ToolResponse(content=[TextBlock(type="text", text=payload)])

        retrieve.__name__ = self.config.retrieve_tool_name
        retrieve.__qualname__ = self.config.retrieve_tool_name
        retrieve.__doc__ = _RETRIEVE_DESCRIPTION
        return retrieve

    def _retrieve_schema(self) -> dict[str, Any]:
        return {
            "type": "function",
            "function": {
                "name": self.config.retrieve_tool_name,
                "description": _RETRIEVE_DESCRIPTION,
                "parameters": {
                    "type": "object",
                    "properties": {
                        "hash": {
                            "type": "string",
                            "pattern": "^[0-9a-fA-F]{24}$",
                            "description": (
                                "The 24-character hash from a "
                                "<<tokenless:HASH>> marker."
                            ),
                        },
                    },
                    "required": ["hash"],
                    "additionalProperties": False,
                },
            },
        }

    @staticmethod
    def _looks_like_error(response: ToolResponse) -> bool:
        return any(
            block.get("type") == "text"
            and block.get("text", "").lstrip().startswith("Error:")
            for block in response.content
        )
