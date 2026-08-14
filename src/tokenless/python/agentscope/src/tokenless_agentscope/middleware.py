"""AgentScope middleware for reversible Tokenless response compression."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
from collections.abc import AsyncGenerator, Callable, Collection
from enum import StrEnum
from pathlib import Path
from typing import Any, ClassVar

from agentscope.message import TextBlock, ToolResultState
from agentscope.middleware import MiddlewareBase
from agentscope.permission import PermissionBehavior, PermissionDecision
from agentscope.tool import ToolBase, ToolChunk, Toolkit, ToolResponse
from anolisa_tokenless import TokenlessError, TokenlessRuntime

logger = logging.getLogger(__name__)

_RETRIEVE_TOOL_NAME = "tokenless_retrieve"
_HASH_PATTERN = re.compile(r"^[0-9a-fA-F]{24}$")

# Keep these names aligned with common/hooks/tool_categories.json. The integration
# package is installable on its own, so it cannot load that sibling resource at
# runtime; a repository test guards the copied policy against drift.
_SKIP_TOOLS = frozenset(
    {
        "Read",
        "read",
        "read_file",
        "read_many_files",
        "Glob",
        "glob",
        "search_file",
        "list_directory",
        "list_dir",
        "Grep",
        "grep",
        "grep_code",
        "grep_search",
        "search_files",
        "Lsp",
        "lsp",
        "NotebookRead",
        "notebook_read",
        "notebookread",
    },
)
_SHELL_TOOLS = frozenset(
    {
        "Bash",
        "bash",
        "Shell",
        "shell",
        "exec",
        "terminal",
        "run_shell_command",
        "run_in_terminal",
        "get_terminal_output",
        "execute_command",
        "process",
    },
)

_CONSERVATIVE_THRESHOLDS = (1_048_576, 65_536, 32)
_SHELL_THRESHOLDS = (65_536, 128, 8)
_AGGRESSIVE_THRESHOLDS = (4_096, 32, 8)


class CompressionMode(StrEnum):
    """Supported AgentScope response-compression policies."""

    CONSERVATIVE = "conservative"
    BALANCED = "balanced"
    AGGRESSIVE = "aggressive"


def _state_contains_marker(state: Any, hash_value: str) -> bool:
    """Return whether the current context or summary contains the marker."""
    marker = f"<<tokenless:{hash_value.lower()}>>"
    serialized = state.model_dump_json(include={"context", "summary"})
    return marker in serialized.lower()


class _TokenlessRetrieveTool(ToolBase):
    """Narrow, read-only AgentScope tool for recovering stashed content."""

    name = _RETRIEVE_TOOL_NAME
    description = (
        "Recover content omitted at a <<tokenless:HASH>> marker. Call this "
        "only when the omitted content is necessary, passing the marker's "
        "24-character hexadecimal HASH."
    )
    input_schema: ClassVar[dict[str, Any]] = {
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
    is_concurrency_safe = True
    is_read_only = True
    is_state_injected = True
    is_external_tool = False
    is_mcp = False
    mcp_name = None

    def __init__(self, runtime: TokenlessRuntime, name: str) -> None:
        super().__init__()
        self._runtime = runtime
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

    async def call(self, hash: str, _agent_state: Any) -> ToolChunk:
        """Retrieve the exact payload referenced from the current context."""
        if not isinstance(hash, str) or _HASH_PATTERN.fullmatch(hash) is None:
            return self._error_chunk(
                "Invalid Tokenless stash hash; expected exactly 24 hexadecimal characters.",
            )

        normalized = hash.lower()
        if not _state_contains_marker(_agent_state, normalized):
            return self._error_chunk(
                "The requested Tokenless marker is not present in the current session context.",
            )

        try:
            payload = await asyncio.to_thread(self._runtime.retrieve, normalized)
        except TokenlessError as error:
            return self._error_chunk(str(error))
        return ToolChunk(content=[TextBlock(text=payload)])

    @staticmethod
    def _error_chunk(message: str) -> ToolChunk:
        return ToolChunk(
            content=[TextBlock(text=message)],
            state=ToolResultState.ERROR,
        )


class TokenlessMiddleware(MiddlewareBase):
    """Compress final AgentScope tool responses with the Tokenless runtime."""

    def __init__(
        self,
        *,
        mode: CompressionMode | str = CompressionMode.BALANCED,
        data_dir: str | os.PathLike[str] | None = None,
        min_chars: int = 200,
        excluded_tools: Collection[str] = (),
        retrieve_tool_name: str = _RETRIEVE_TOOL_NAME,
    ) -> None:
        """Configure response compression and its paired retrieval tool."""
        self.mode = CompressionMode(mode)
        if min_chars < 0:
            raise ValueError("min_chars must be non-negative")
        if not retrieve_tool_name:
            raise ValueError("retrieve_tool_name must not be empty")

        if data_dir is None:
            self.data_dir = None
        else:
            resolved_data_dir = Path(data_dir).expanduser()
            if not resolved_data_dir.is_absolute():
                raise ValueError("data_dir must be an absolute path")
            self.data_dir = os.fspath(resolved_data_dir)
        self.min_chars = min_chars
        self.retrieve_tool_name = retrieve_tool_name
        self.excluded_tools = frozenset(excluded_tools) | {
            retrieve_tool_name,
        }
        self._runtime = TokenlessRuntime(self.data_dir)
        self._retrieve_tool = _TokenlessRetrieveTool(
            self._runtime,
            retrieve_tool_name,
        )

    async def list_tools(self) -> list[ToolBase]:
        """Return the retrieval tool for AgentScope App toolkit assembly."""
        return [self._retrieve_tool]

    async def register_tools(self, toolkit: Toolkit) -> None:
        """Register retrieval in a high-code Toolkit without name hijacking."""
        existing = await toolkit.get_tool(self.retrieve_tool_name)
        if existing is self._retrieve_tool:
            return
        if existing is not None:
            raise ValueError(
                f"Toolkit already contains a different '{self.retrieve_tool_name}' tool",
            )
        await toolkit.add_tool(self._retrieve_tool)

    async def on_acting(
        self,
        agent: Any,
        input_kwargs: dict[str, Any],
        next_handler: Callable[..., AsyncGenerator[Any, None]],
    ) -> AsyncGenerator[Any, None]:
        """Pass chunks through and compress only the successful final response."""
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
        content = [replacements.get(index, block) for index, block in enumerate(response.content)]
        return response.model_copy(update={"content": content})

    async def _compress_text(
        self,
        text: str,
        *,
        tool_name: str,
        session_id: str,
        tool_use_id: str,
    ) -> str | None:
        input_text, expected_type = self._normalize_input(text)
        thresholds = self._thresholds_for(tool_name)
        try:
            result = await asyncio.to_thread(
                self._runtime.compress_response,
                input_text,
                truncate_strings_at=thresholds[0],
                truncate_arrays_at=thresholds[1],
                max_depth=thresholds[2],
                agent_id="agentscope",
                session_id=session_id,
                tool_use_id=tool_use_id,
                require_reversible=True,
            )
        except TokenlessError as error:
            logger.warning("Tokenless compression failed: %s", error)
            return None
        if not result.applied:
            return None

        try:
            candidate_json = result.output
            candidate_value = json.loads(candidate_json)
        except json.JSONDecodeError:
            logger.warning("Tokenless returned an invalid compression result")
            return None

        if expected_type is str:
            if not isinstance(candidate_value, str):
                return None
            candidate = candidate_value
        else:
            if type(candidate_value) is not expected_type:
                return None
            candidate = candidate_json.strip()

        if len(candidate.encode("utf-8")) >= len(text.encode("utf-8")):
            return None
        return candidate

    @staticmethod
    def _normalize_input(text: str) -> tuple[str, type[Any]]:
        try:
            value = json.loads(text)
        except json.JSONDecodeError:
            value = None
        if isinstance(value, (dict, list)):
            return text, type(value)
        return json.dumps(text, ensure_ascii=False), str

    def _thresholds_for(self, tool_name: str) -> tuple[int, int, int]:
        if self.mode is CompressionMode.AGGRESSIVE:
            return _AGGRESSIVE_THRESHOLDS
        if self.mode is CompressionMode.BALANCED and tool_name in _SHELL_TOOLS:
            return _SHELL_THRESHOLDS
        return _CONSERVATIVE_THRESHOLDS

    def _is_excluded(self, tool_name: str) -> bool:
        if tool_name in self.excluded_tools:
            return True
        return self.mode is not CompressionMode.CONSERVATIVE and tool_name in _SKIP_TOOLS
