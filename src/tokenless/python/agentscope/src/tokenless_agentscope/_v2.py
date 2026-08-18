"""AgentScope 2.x integration for the complete Tokenless lifecycle."""

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
    Attribution,
    CompressionMode,
    ModelRequest,
    RetrievalError,
    RetrieveRequest,
    TokenlessConfig,
    TokenlessSdk,
    ToolCall,
    ToolResult,
    ToolStatus,
)
from anolisa_tokenless.tool_response import (
    AGGRESSIVE_THRESHOLDS,
    CONSERVATIVE_THRESHOLDS,
    SHELL_THRESHOLDS,
    SHELL_TOOLS,
    SKIP_TOOLS,
)

_STATE_KEY = "anolisa_tokenless"
_RETRIEVE_DESCRIPTION = (
    "Recover content omitted at a <<tokenless:HASH>> marker. Call this only "
    "when the omitted content is necessary."
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

_SKIP_TOOLS = SKIP_TOOLS
_SHELL_TOOLS = SHELL_TOOLS
_CONSERVATIVE_THRESHOLDS = CONSERVATIVE_THRESHOLDS
_SHELL_THRESHOLDS = SHELL_THRESHOLDS
_AGGRESSIVE_THRESHOLDS = AGGRESSIVE_THRESHOLDS


def _marker_state(
    state: Any, session_markers: dict[str, frozenset[str]]
) -> frozenset[str]:
    middle_context = getattr(state, "middle_context", None)
    if middle_context is not None:
        values = middle_context.get(_STATE_KEY, {}).get("visible_markers", [])
        return frozenset(values)
    return session_markers.get(state.session_id, frozenset())


def _set_marker_state(
    state: Any,
    markers: frozenset[str],
    session_markers: dict[str, frozenset[str]],
) -> None:
    middle_context = getattr(state, "middle_context", None)
    if middle_context is not None:
        middle_context[_STATE_KEY] = {"visible_markers": sorted(markers)}
    else:
        session_markers[state.session_id] = markers


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

    def __init__(
        self,
        sdk: TokenlessSdk,
        name: str,
        session_markers: dict[str, frozenset[str]],
    ) -> None:
        super().__init__()
        self._sdk = sdk
        self.name = name
        self._session_markers = session_markers

    async def check_permissions(
        self, tool_input: dict[str, Any], context: Any
    ) -> PermissionDecision:
        del tool_input, context
        return PermissionDecision(
            behavior=PermissionBehavior.ALLOW,
            message="Tokenless retrieval is a read-only, marker-scoped operation.",
        )

    async def _retrieve(self, hash_value: str, state: Any) -> ToolChunk:
        attribution = Attribution("agentscope", state.session_id)
        try:
            payload = await self._sdk.retrieve(
                RetrieveRequest(
                    hash_value,
                    _marker_state(state, self._session_markers),
                    attribution,
                )
            )
        except RetrievalError as error:
            return ToolChunk(
                content=[TextBlock(text=str(error))],
                state=ToolResultState.ERROR,
            )
        return ToolChunk(content=[TextBlock(text=payload)])


class _LegacyRetrieveTool(_RetrieveToolMixin, ToolBase):
    """Retrieval tool for AgentScope 2.0.0 through 2.0.2."""

    async def __call__(self, hash: str, _agent_state: Any) -> ToolChunk:
        return await self._retrieve(hash, _agent_state)


class _ModernRetrieveTool(_RetrieveToolMixin, ToolBase):
    """Retrieval tool for AgentScope 2.0.3 and later."""

    async def call(self, hash: str, _agent_state: Any) -> ToolChunk:
        return await self._retrieve(hash, _agent_state)


def _new_retrieve_tool(
    sdk: TokenlessSdk,
    name: str,
    session_markers: dict[str, frozenset[str]],
) -> ToolBase:
    tool_type = (
        _ModernRetrieveTool if hasattr(ToolBase, "call") else _LegacyRetrieveTool
    )
    return tool_type(sdk, name, session_markers)


class TokenlessMiddleware(MiddlewareBase):
    """Apply all Tokenless lifecycles to AgentScope 2.x."""

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
        self.sdk = TokenlessSdk(config)
        self._runtime = self.sdk.runtime
        self._session_markers: dict[str, frozenset[str]] = {}
        self._retrieve_tool = _new_retrieve_tool(
            self.sdk,
            config.retrieve_tool_name,
            self._session_markers,
        )

    @property
    def retrieve_tool(self) -> ToolBase:
        """Return the retrieval tool paired with this middleware runtime."""
        return self._retrieve_tool

    async def list_tools(self) -> list[ToolBase]:
        """Publish retrieval while model middleware controls its visibility."""
        return [self._retrieve_tool] if self._publish_retrieval_tool else []

    async def register_tools(self, toolkit: Toolkit) -> None:
        """Register retrieval when the installed Toolkit supports mutation."""
        existing = await toolkit.get_tool(self.retrieve_tool_name)
        if existing is self._retrieve_tool:
            return
        if existing is not None:
            raise ValueError(
                f"Toolkit already contains a different '{self.retrieve_tool_name}' tool"
            )
        add_tool = getattr(toolkit, "add_tool", None)
        if add_tool is None:
            raise RuntimeError(
                "This AgentScope Toolkit cannot be mutated; construct it with "
                "Toolkit(tools=[..., middleware.retrieve_tool])."
            )
        await add_tool(self._retrieve_tool)

    async def on_model_call(
        self,
        agent: Any,
        input_kwargs: dict[str, Any],
        next_handler: Callable[..., Any],
    ) -> Any:
        """Compress schemas and retain the exact marker authorization set."""
        request = ModelRequest(
            tools=tuple(input_kwargs["tools"]),
            visible_context=json.dumps(
                input_kwargs["messages"], ensure_ascii=False, default=str
            ),
            attribution=Attribution(str(agent.name), agent.state.session_id),
            visible_markers=_marker_state(agent.state, self._session_markers),
        )
        transformed = await self.sdk.before_model(request)
        _set_marker_state(
            agent.state,
            transformed.visible_markers,
            self._session_markers,
        )
        return await next_handler(**{**input_kwargs, "tools": list(transformed.tools)})

    async def on_acting(
        self,
        agent: Any,
        input_kwargs: dict[str, Any],
        next_handler: Callable[..., AsyncGenerator[Any, None]],
    ) -> AsyncGenerator[Any, None]:
        """Rewrite copied calls and transform only their final response."""
        source = input_kwargs["tool_call"]
        arguments = json.loads(source.input)
        if not isinstance(arguments, dict):
            raise TypeError("AgentScope tool input must decode to a JSON object")
        command_field = (
            "command"
            if source.name in SHELL_TOOLS and isinstance(arguments.get("command"), str)
            else None
        )
        call = ToolCall(
            source.name,
            arguments,
            Attribution(str(agent.name), agent.state.session_id, source.id),
            command_field=command_field,
        )
        transformed_call = await self.sdk.before_tool_call(call)
        forwarded = source.model_copy(
            update={
                "input": json.dumps(
                    transformed_call.arguments,
                    ensure_ascii=False,
                    separators=(",", ":"),
                )
            }
        )
        async for item in next_handler(**{**input_kwargs, "tool_call": forwarded}):
            if isinstance(item, ToolResponse):
                yield await self._after_response(item, transformed_call)
            else:
                yield item

    async def _after_response(
        self, response: ToolResponse, call: ToolCall
    ) -> ToolResponse:
        status = self._status(response.state)
        replacements: dict[int, TextBlock] = {}
        extra_context: str | None = None
        for index, block in enumerate(response.content):
            if not isinstance(block, TextBlock):
                continue
            transformed = await self.sdk.after_tool_call(
                ToolResult(call, block.text, status)
            )
            extra_context = extra_context or transformed.additional_context
            if transformed.transformed:
                replacements[index] = block.model_copy(
                    update={"text": transformed.content}
                )
        content = [
            replacements.get(index, block)
            for index, block in enumerate(response.content)
        ]
        if extra_context is not None:
            content.append(TextBlock(text=extra_context))
        if content == response.content:
            return response
        return response.model_copy(update={"content": content})

    def _thresholds_for(self, tool_name: str) -> tuple[int, int, int]:
        return self.sdk._response.thresholds_for(tool_name)

    def _is_excluded(self, tool_name: str) -> bool:
        return self.sdk._response.is_excluded(tool_name)

    @staticmethod
    def _status(state: ToolResultState) -> ToolStatus:
        mapping = {
            ToolResultState.SUCCESS: ToolStatus.SUCCESS,
            ToolResultState.ERROR: ToolStatus.ERROR,
            ToolResultState.INTERRUPTED: ToolStatus.INTERRUPTED,
            ToolResultState.DENIED: ToolStatus.DENIED,
        }
        return mapping.get(state, ToolStatus.INTERRUPTED)


class TokenlessAgentScope:
    """Stable Tokenless entry point for AgentScope 2.x applications."""

    def __init__(self, config: TokenlessConfig | None = None) -> None:
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
                "use direct Agent construction or AgentScope 2.0.1 or later."
            )
        if self.config.data_dir is None:
            raise ValueError("TokenlessConfig.data_dir is required for AgentScope App")

        async def middleware_factory(
            user_id: str, agent_id: str, session_id: str
        ) -> list[MiddlewareBase]:
            config = replace(
                self.config,
                data_dir=self._app_data_dir(user_id, agent_id, session_id),
            )
            return [TokenlessMiddleware(_config=config, _publish_retrieval_tool=False)]

        async def tool_factory(
            user_id: str, agent_id: str, session_id: str
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
            [user_id, agent_id, session_id], ensure_ascii=False, separators=(",", ":")
        )
        key = hashlib.sha256(identity.encode()).hexdigest()
        return Path(self.config.data_dir) / "agentscope-app" / key
