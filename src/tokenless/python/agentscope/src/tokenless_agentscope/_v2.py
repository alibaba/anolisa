"""AgentScope 2.x integration for reversible Tokenless compression."""

from __future__ import annotations

import hashlib
import json
import os
from collections.abc import AsyncGenerator, Callable, Collection
from dataclasses import replace
from pathlib import Path
from typing import Any, ClassVar

from agentscope.message import TextBlock, ToolResultState
from agentscope.middleware import MiddlewareBase
from agentscope.permission import PermissionBehavior, PermissionDecision
from agentscope.tool import ToolBase, ToolChunk, Toolkit, ToolResponse
from anolisa_tokenless import (
    CompressionMode,
    RetrievalError,
    TokenlessConfig,
    ToolResponseCompressor,
)
from anolisa_tokenless.tool_response import (
    AGGRESSIVE_THRESHOLDS,
    CONSERVATIVE_THRESHOLDS,
    SHELL_THRESHOLDS,
    SHELL_TOOLS,
    SKIP_TOOLS,
)

_RETRIEVE_DESCRIPTION = (
    "Recover content omitted at a <<tokenless:HASH>> marker. Call this "
    "only when the omitted content is necessary, passing the marker's "
    "24-character hexadecimal HASH."
)
_RETRIEVE_SCHEMA: dict[str, Any] = {
    "type": "object",
    "properties": {
        "hash": {
            "type": "string",
            "pattern": "^[0-9a-fA-F]{24}$",
            "description": "The 24-character hash from a <<tokenless:HASH>> marker.",
        },
    },
    "required": ["hash"],
    "additionalProperties": False,
}

# Retain private aliases used by the policy drift test and existing callers
# that inspected the old middleware module.
_SKIP_TOOLS = SKIP_TOOLS
_SHELL_TOOLS = SHELL_TOOLS
_CONSERVATIVE_THRESHOLDS = CONSERVATIVE_THRESHOLDS
_SHELL_THRESHOLDS = SHELL_THRESHOLDS
_AGGRESSIVE_THRESHOLDS = AGGRESSIVE_THRESHOLDS


def _state_visible_context(state: Any) -> str:
    """Serialize only AgentScope state visible to the current model."""
    return state.model_dump_json(include={"context", "summary"})


class _RetrieveToolMixin:
    """Shared retrieval behavior across the AgentScope 2.x Tool ABI change."""

    name = "tokenless_retrieve"
    description = _RETRIEVE_DESCRIPTION
    input_schema: ClassVar[dict[str, Any]] = _RETRIEVE_SCHEMA
    is_concurrency_safe = True
    is_read_only = True
    is_state_injected = True
    is_external_tool = False
    is_mcp = False
    mcp_name = None

    def __init__(self, compressor: ToolResponseCompressor, name: str) -> None:
        super().__init__()
        self._compressor = compressor
        self.name = name

    async def check_permissions(
        self,
        tool_input: dict[str, Any],
        context: Any,
    ) -> PermissionDecision:
        """Allow marker-scoped reads without granting general shell access."""
        del tool_input, context
        return PermissionDecision(
            behavior=PermissionBehavior.ALLOW,
            message="Tokenless retrieval is a read-only, marker-scoped operation.",
        )

    async def _retrieve(self, hash_value: str, state: Any) -> ToolChunk:
        try:
            payload = await self._compressor.retrieve(
                hash_value,
                _state_visible_context(state),
            )
        except RetrievalError as error:
            return self._error_chunk(str(error))
        return ToolChunk(content=[TextBlock(text=payload)])

    @staticmethod
    def _error_chunk(message: str) -> ToolChunk:
        return ToolChunk(
            content=[TextBlock(text=message)],
            state=ToolResultState.ERROR,
        )


class _LegacyRetrieveTool(_RetrieveToolMixin, ToolBase):
    """Retrieval tool for AgentScope 2.0.0 through 2.0.2."""

    async def __call__(self, hash: str, _agent_state: Any) -> ToolChunk:
        """Retrieve the exact payload referenced from the current context."""
        return await self._retrieve(hash, _agent_state)


class _ModernRetrieveTool(_RetrieveToolMixin, ToolBase):
    """Retrieval tool for AgentScope 2.0.3 and later."""

    async def call(self, hash: str, _agent_state: Any) -> ToolChunk:
        """Retrieve the exact payload referenced from the current context."""
        return await self._retrieve(hash, _agent_state)


def _new_retrieve_tool(
    compressor: ToolResponseCompressor,
    name: str,
) -> ToolBase:
    """Select the installed AgentScope Tool override point by capability."""
    tool_type = (
        _ModernRetrieveTool if hasattr(ToolBase, "call") else _LegacyRetrieveTool
    )
    return tool_type(compressor, name)


class TokenlessMiddleware(MiddlewareBase):
    """Compress final AgentScope 2.x tool responses with Tokenless."""

    def __init__(
        self,
        *,
        mode: CompressionMode | str = CompressionMode.BALANCED,
        data_dir: str | os.PathLike[str] | None = None,
        min_chars: int = 200,
        excluded_tools: Collection[str] = (),
        retrieve_tool_name: str = "tokenless_retrieve",
        _config: TokenlessConfig | None = None,
        _publish_retrieval_tool: bool = True,
    ) -> None:
        """Configure response compression and its paired retrieval tool."""
        config = _config or TokenlessConfig(
            mode=mode,
            data_dir=data_dir,
            min_chars=min_chars,
            excluded_tools=excluded_tools,
            retrieve_tool_name=retrieve_tool_name,
        )
        self.config = config
        self.mode = config.mode
        self.data_dir = config.data_dir
        self.min_chars = config.min_chars
        self.retrieve_tool_name = config.retrieve_tool_name
        self.excluded_tools = config.excluded_tools
        self._publish_retrieval_tool = _publish_retrieval_tool
        self._compressor = ToolResponseCompressor(config)
        self._runtime = self._compressor.runtime
        self._retrieve_tool = _new_retrieve_tool(
            self._compressor,
            config.retrieve_tool_name,
        )

    @property
    def retrieve_tool(self) -> ToolBase:
        """Return the retrieval tool paired with this middleware runtime."""
        return self._retrieve_tool

    async def list_tools(self) -> list[ToolBase]:
        """Return retrieval for AgentScope versions that collect middleware tools."""
        return [self._retrieve_tool] if self._publish_retrieval_tool else []

    async def register_tools(self, toolkit: Toolkit) -> None:
        """Register retrieval when the installed Toolkit supports mutation."""
        existing = await toolkit.get_tool(self.retrieve_tool_name)
        if existing is self._retrieve_tool:
            return
        if existing is not None:
            raise ValueError(
                f"Toolkit already contains a different '{self.retrieve_tool_name}' tool",
            )
        add_tool = getattr(toolkit, "add_tool", None)
        if add_tool is None:
            raise RuntimeError(
                "This AgentScope Toolkit cannot be mutated; construct it with "
                "Toolkit(tools=[..., middleware.retrieve_tool]).",
            )
        await add_tool(self._retrieve_tool)

    async def on_acting(
        self,
        agent: Any,
        input_kwargs: dict[str, Any],
        next_handler: Callable[..., AsyncGenerator[Any, None]],
    ) -> AsyncGenerator[Any, None]:
        """Pass chunks through and compress only successful final responses."""
        tool_call = input_kwargs["tool_call"]
        tool_name = tool_call.name
        async for item in next_handler(**input_kwargs):
            if (
                not self._is_excluded(tool_name)
                and isinstance(item, ToolResponse)
                and item.state == ToolResultState.SUCCESS
            ):
                yield await self._compress_response(
                    item,
                    tool_name=tool_name,
                    session_id=agent.state.session_id,
                    tool_use_id=tool_call.id,
                )
            else:
                yield item

    async def _compress_response(
        self,
        response: ToolResponse,
        *,
        tool_name: str,
        session_id: str,
        tool_use_id: str,
    ) -> ToolResponse:
        replacements: dict[int, TextBlock] = {}
        for index, block in enumerate(response.content):
            if not isinstance(block, TextBlock) or len(block.text) < self.min_chars:
                continue
            compressed = await self._compress_text(
                block.text,
                tool_name=tool_name,
                session_id=session_id,
                tool_use_id=tool_use_id,
            )
            if compressed is not None:
                replacements[index] = block.model_copy(update={"text": compressed})
        if not replacements:
            return response
        content = [
            replacements.get(index, block)
            for index, block in enumerate(response.content)
        ]
        return response.model_copy(update={"content": content})

    async def _compress_text(
        self,
        text: str,
        *,
        tool_name: str,
        session_id: str,
        tool_use_id: str,
    ) -> str | None:
        return await self._compressor.compress_text(
            text,
            tool_name=tool_name,
            agent_id="agentscope",
            session_id=session_id,
            tool_use_id=tool_use_id,
        )

    def _thresholds_for(self, tool_name: str) -> tuple[int, int, int]:
        return self._compressor.thresholds_for(tool_name)

    def _is_excluded(self, tool_name: str) -> bool:
        return self._compressor.is_excluded(tool_name)


class TokenlessAgentScope:
    """Stable Tokenless entry point for AgentScope 2.x applications."""

    def __init__(self, config: TokenlessConfig | None = None) -> None:
        """Create the tools and middlewares to pass during Agent construction."""
        self.config = config or TokenlessConfig()
        self.middleware = TokenlessMiddleware(_config=self.config)

    @property
    def tools(self) -> list[ToolBase]:
        """Return tools to include in ``Toolkit(tools=...)``."""
        return [self.middleware.retrieve_tool]

    @property
    def middlewares(self) -> list[MiddlewareBase]:
        """Return middlewares to include in the Agent constructor."""
        return [self.middleware]

    def app_options(self) -> dict[str, Callable[..., Any]]:
        """Return AgentScope App factories with isolated session storage."""
        if not hasattr(MiddlewareBase, "list_tools"):
            raise RuntimeError(
                "AgentScope 2.0.0 App cannot inject Agent middleware and tools; "
                "use direct Agent construction or AgentScope 2.0.1 or later.",
            )
        if self.config.data_dir is None:
            raise ValueError("TokenlessConfig.data_dir is required for AgentScope App")

        async def middleware_factory(
            user_id: str,
            agent_id: str,
            session_id: str,
        ) -> list[MiddlewareBase]:
            config = replace(
                self.config,
                data_dir=self._app_data_dir(user_id, agent_id, session_id),
            )
            return [
                TokenlessMiddleware(
                    _config=config,
                    _publish_retrieval_tool=False,
                ),
            ]

        async def tool_factory(
            user_id: str,
            agent_id: str,
            session_id: str,
        ) -> list[ToolBase]:
            config = replace(
                self.config,
                data_dir=self._app_data_dir(user_id, agent_id, session_id),
            )
            middleware = TokenlessMiddleware(_config=config)
            return [middleware.retrieve_tool]

        return {
            "extra_agent_middlewares": middleware_factory,
            "extra_agent_tools": tool_factory,
        }

    def _app_data_dir(self, user_id: str, agent_id: str, session_id: str) -> Path:
        identity = json.dumps(
            [user_id, agent_id, session_id],
            ensure_ascii=False,
            separators=(",", ":"),
        )
        key = hashlib.sha256(identity.encode("utf-8")).hexdigest()
        return Path(self.config.data_dir) / "agentscope-app" / key
