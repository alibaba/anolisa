"""AgentScope 1.x integration for the complete Tokenless lifecycle."""

from __future__ import annotations

import copy
import inspect
import json
from dataclasses import replace
from typing import Any

from agentscope.message import TextBlock
from agentscope.tool import Toolkit, ToolResponse
from anolisa_tokenless import (
    Attribution,
    ModelRequest,
    RetrievalError,
    RetrieveRequest,
    TokenlessConfig,
    TokenlessSdk,
    ToolCall,
    ToolResult,
    ToolStatus,
)
from anolisa_tokenless.tool_response import SHELL_TOOLS


class _TokenlessToolkit(Toolkit):
    """Toolkit that applies Tokenless to current and future registrations."""

    def __init__(self, integration: TokenlessAgentScope) -> None:
        super().__init__()
        self._integration = integration
        self._agent: Any | None = None
        self._session_id: str | None = None
        self.visible_markers: frozenset[str] = frozenset()
        self.rewritten_calls: set[str] = set()
        self._retrieve_function: Any | None = None

    def bind(self, agent: Any, session_id: str) -> None:
        """Bind execution attribution after the Agent is constructed."""
        self._agent = agent
        self._session_id = session_id

    def register_tool_function(self, tool_func: Any, *args: Any, **kwargs: Any) -> None:
        """Chain application postprocessing with Tokenless for every tool."""
        arguments = (
            inspect.signature(super().register_tool_function)
            .bind_partial(tool_func, *args, **kwargs)
            .arguments
        )
        variadic = arguments.get("kwargs")
        if isinstance(variadic, dict):
            arguments = {**variadic, **arguments}
        schema = arguments.get("json_schema")
        name = (
            schema.get("function", {}).get("name")
            if isinstance(schema, dict)
            else arguments.get("func_name") or getattr(tool_func, "__name__", None)
        )
        if name == self._integration.config.retrieve_tool_name:
            if (
                self._retrieve_function is not None
                and tool_func is not self._retrieve_function
            ):
                raise ValueError(
                    f"Tool name {name!r} is reserved for Tokenless retrieval"
                )
        else:
            kwargs["postprocess_func"] = self._wrap_postprocessor(
                kwargs.get("postprocess_func")
            )
        super().register_tool_function(tool_func, *args, **kwargs)
        if name == self._integration.config.retrieve_tool_name:
            self._retrieve_function = tool_func

    def _wrap_postprocessor(self, previous: Any) -> Any:
        async def postprocess(
            tool_call: dict[str, Any], response: ToolResponse
        ) -> ToolResponse:
            if previous is not None:
                processed = previous(tool_call, response)
                if inspect.isawaitable(processed):
                    processed = await processed
                if processed is not None:
                    response = processed
            return await self._integration._after_tool(self, tool_call, response)

        return postprocess


class _ModelProxy:
    """Delegate an AgentScope model while transforming only its tool schemas."""

    def __init__(
        self, integration: TokenlessAgentScope, toolkit: _TokenlessToolkit, model: Any
    ):
        object.__setattr__(self, "_integration", integration)
        object.__setattr__(self, "_toolkit", toolkit)
        object.__setattr__(self, "_model", model)

    async def __call__(self, *args: Any, **kwargs: Any) -> Any:
        tools = kwargs.get("tools")
        if tools is not None:
            prompt = args[0] if args else kwargs.get("prompt", "")
            request = ModelRequest(
                tools=tuple(tools),
                visible_context=json.dumps(prompt, ensure_ascii=False, default=str),
                attribution=self._integration._attribution(),
                visible_markers=self._toolkit.visible_markers,
            )
            transformed = await self._integration.sdk.before_model(request)
            kwargs = dict(kwargs)
            kwargs["tools"] = list(transformed.tools)
            self._toolkit.visible_markers = transformed.visible_markers
        return await self._model(*args, **kwargs)

    def __getattr__(self, name: str) -> Any:
        return getattr(self._model, name)

    def __setattr__(self, name: str, value: Any) -> None:
        if name in {"_integration", "_toolkit", "_model"}:
            object.__setattr__(self, name, value)
        else:
            setattr(self._model, name, value)


class TokenlessAgentScope:
    """Stable Tokenless entry point for AgentScope 1.x agents."""

    def __init__(self, config: TokenlessConfig | None = None) -> None:
        self.config = config or TokenlessConfig()
        self.sdk = TokenlessSdk(self.config)
        self._installed_agent: Any | None = None
        self._session_id: str | None = None

    def create_toolkit(self) -> Toolkit:
        """Create the required Toolkit and register marker-scoped retrieval."""
        toolkit = _TokenlessToolkit(self)
        retrieve = self._build_retrieve_tool(toolkit)
        toolkit.register_tool_function(
            retrieve,
            json_schema=self.sdk.retrieve_schema(),
        )
        return toolkit

    def install(self, agent: Any, *, session_id: str) -> None:
        """Bind one Agent, its model boundary, and acting hook."""
        if not session_id:
            raise ValueError("session_id must not be empty")
        if self._installed_agent is agent:
            if self._session_id != session_id:
                raise ValueError(
                    "Tokenless session_id cannot change after installation"
                )
            return
        if self._installed_agent is not None:
            raise ValueError(
                "A TokenlessAgentScope instance can be installed on only one Agent"
            )
        if not isinstance(getattr(agent, "toolkit", None), _TokenlessToolkit):
            raise TypeError("AgentScope 1.x requires integration.create_toolkit()")
        if not hasattr(agent, "model") or not hasattr(agent, "register_instance_hook"):
            raise TypeError(
                "AgentScope 1.x integration requires a ReActAgent-compatible object"
            )

        toolkit = agent.toolkit
        if toolkit._integration is not self:
            raise ValueError(
                "The Agent toolkit belongs to a different Tokenless integration"
            )
        self._installed_agent = agent
        self._session_id = session_id
        toolkit.bind(agent, session_id)
        agent.model = _ModelProxy(self, toolkit, agent.model)
        agent.register_instance_hook("pre_acting", "tokenless", self._before_acting)

    async def _before_acting(
        self, agent: Any, kwargs: dict[str, Any]
    ) -> dict[str, Any]:
        tool_call = copy.deepcopy(kwargs["tool_call"])
        inputs = tool_call.get("input") or {}
        command_field = (
            "command"
            if tool_call["name"] in SHELL_TOOLS
            and isinstance(inputs.get("command"), str)
            else None
        )
        call = ToolCall(
            tool_call["name"],
            dict(inputs),
            self._attribution(tool_call["id"]),
            command_field=command_field,
        )
        transformed = await self.sdk.before_tool_call(call)
        tool_call["input"] = transformed.arguments
        if transformed.rewritten:
            agent.toolkit.rewritten_calls.add(tool_call["id"])
        return {**kwargs, "tool_call": tool_call}

    async def _after_tool(
        self,
        toolkit: _TokenlessToolkit,
        tool_call: dict[str, Any],
        response: ToolResponse,
    ) -> ToolResponse:
        if not response.is_last:
            return response
        status = self._status(response)
        rewritten = tool_call["id"] in toolkit.rewritten_calls
        call = ToolCall(
            tool_call["name"],
            dict(tool_call.get("input") or {}),
            self._attribution(tool_call["id"]),
            rewritten=rewritten,
        )
        replacements: dict[int, TextBlock] = {}
        extra_context: str | None = None
        for index, block in enumerate(response.content):
            if block.get("type") != "text":
                continue
            transformed = await self.sdk.after_tool_call(
                ToolResult(call, block.get("text", ""), status)
            )
            extra_context = extra_context or transformed.additional_context
            if transformed.transformed:
                replacements[index] = TextBlock(type="text", text=transformed.content)
        toolkit.rewritten_calls.discard(tool_call["id"])
        content = [
            replacements.get(index, block)
            for index, block in enumerate(response.content)
        ]
        if extra_context is not None:
            content.append(TextBlock(type="text", text=extra_context))
        if content == response.content:
            return response
        return replace(response, content=content)

    def _build_retrieve_tool(self, toolkit: _TokenlessToolkit) -> Any:
        async def retrieve(hash: str) -> ToolResponse:
            try:
                payload = await self.sdk.retrieve(
                    RetrieveRequest(hash, toolkit.visible_markers, self._attribution())
                )
            except RetrievalError as error:
                return ToolResponse(
                    content=[TextBlock(type="text", text=f"Error: {error}")]
                )
            return ToolResponse(content=[TextBlock(type="text", text=payload)])

        retrieve.__name__ = self.config.retrieve_tool_name
        retrieve.__qualname__ = self.config.retrieve_tool_name
        retrieve.__doc__ = self.sdk.retrieve_schema()["function"]["description"]
        return retrieve

    def _attribution(self, tool_use_id: str | None = None) -> Attribution:
        if self._installed_agent is None or self._session_id is None:
            raise RuntimeError("TokenlessAgentScope must be installed before use")
        return Attribution(
            str(self._installed_agent.name), self._session_id, tool_use_id
        )

    @staticmethod
    def _status(response: ToolResponse) -> ToolStatus:
        if response.is_interrupted:
            return ToolStatus.INTERRUPTED
        if response.metadata is not None and response.metadata.get("success") is False:
            return ToolStatus.ERROR
        if any(
            block.get("type") == "text"
            and block.get("text", "").lstrip().startswith("Error:")
            for block in response.content
        ):
            return ToolStatus.ERROR
        return ToolStatus.SUCCESS
