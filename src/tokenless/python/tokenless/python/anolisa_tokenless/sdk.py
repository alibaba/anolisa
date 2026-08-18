"""Framework-neutral lifecycle API for complete Tokenless integration."""

from __future__ import annotations

import asyncio
import copy
import json
import logging
import os
import re
import shlex
import subprocess
from dataclasses import dataclass, field, replace
from enum import StrEnum
from importlib.resources import files
from pathlib import Path
from typing import Any

from anolisa_tokenless._native import TokenlessError, TokenlessRuntime
from anolisa_tokenless.tool_response import TokenlessConfig, ToolResponseCompressor

logger = logging.getLogger(__name__)

_HASH_PATTERN = re.compile(r"^[0-9a-fA-F]{24}$")
_MARKER_PATTERN = re.compile(r"<<tokenless:([0-9a-fA-F]{24})>>")
_SEGMENT_OPERATORS = frozenset({"&&", "||", ";", "|", "&"})
_ENV_ERRORS: tuple[tuple[tuple[str, ...], str, str], ...] = (
    (
        ("command not found", "not installed", "cannot execute", "unable to locate"),
        "ENV_DEPENDENCY_MISSING",
        "Install the missing dependency or ask the user for guidance.",
    ),
    (
        ("permission denied", "operation not permitted", "eacces", "access denied"),
        "ENV_PERMISSION",
        "Check file and directory permissions.",
    ),
    (
        ("no such file or directory", "enoent", "file not found", "does not exist"),
        "ENV_FILE_MISSING",
        "Verify the required file or directory path.",
    ),
    (
        (
            "connection refused",
            "network is unreachable",
            "could not resolve host",
            "etimedout",
        ),
        "ENV_NETWORK",
        "Check DNS, proxy, firewall, and network connectivity.",
    ),
    (
        ("modulenotfounderror", "no module named", "cannot import name"),
        "ENV_PACKAGE_MISSING",
        "Install the required package or module.",
    ),
)

_RETRIEVE_DESCRIPTION = (
    "Recover content omitted at a <<tokenless:HASH>> marker. Call this only "
    "when the omitted content is necessary."
)


class ToolStatus(StrEnum):
    """Framework-neutral final state of one tool execution."""

    SUCCESS = "success"
    ERROR = "error"
    INTERRUPTED = "interrupted"
    DENIED = "denied"


@dataclass(frozen=True)
class Attribution:
    """Stable identifiers used to attribute transformations and retrieval."""

    agent_id: str
    session_id: str
    tool_use_id: str | None = None

    def __post_init__(self) -> None:
        if not self.agent_id:
            raise ValueError("agent_id must not be empty")
        if not self.session_id:
            raise ValueError("session_id must not be empty")


@dataclass(frozen=True)
class ModelRequest:
    """Model-visible tools and context at the pre-model boundary.

    ``visible_markers`` is output state: ``before_model`` replaces any input
    value with the exact markers visible in the transformed request.
    """

    tools: tuple[dict[str, Any], ...]
    visible_context: str
    attribution: Attribution
    visible_markers: frozenset[str] = field(default_factory=frozenset)


@dataclass(frozen=True)
class ToolCall:
    """One tool call before framework execution."""

    name: str
    arguments: dict[str, Any]
    attribution: Attribution
    command_field: str | None = None
    rewritten: bool = False

    def __post_init__(self) -> None:
        if not self.name:
            raise ValueError("tool name must not be empty")
        if self.attribution.tool_use_id is None:
            raise ValueError("tool_use_id is required for a tool lifecycle")


@dataclass(frozen=True)
class ToolResult:
    """Model-visible final text produced by one tool call."""

    call: ToolCall
    content: str
    status: ToolStatus
    additional_context: str | None = None
    transformed: bool = False

    def __post_init__(self) -> None:
        object.__setattr__(self, "status", ToolStatus(self.status))


@dataclass(frozen=True)
class RetrieveRequest:
    """Marker-authorized stash lookup with framework correlation identity."""

    hash: str
    visible_markers: frozenset[str]
    attribution: Attribution


class TokenlessSdk:
    """Apply all Tokenless capabilities at four framework lifecycle seams."""

    def __init__(self, config: TokenlessConfig | None = None) -> None:
        self.config = config or TokenlessConfig()
        self.runtime = TokenlessRuntime(self.config.data_dir)
        self._response = ToolResponseCompressor(self.config, runtime=self.runtime)
        self._rtk_path = self._resolve_rtk() if self.config.rtk_enabled else None

    async def before_model(self, request: ModelRequest) -> ModelRequest:
        """Compress function schemas and publish retrieve only when usable."""
        tools: list[dict[str, Any]] = []
        retrieve_schema = self.retrieve_schema()
        retrieve_seen = False
        for tool in request.tools:
            candidate = copy.deepcopy(tool)
            if candidate.get("type") != "function":
                tools.append(candidate)
                continue
            function = candidate.get("function")
            if not isinstance(function, dict) or not isinstance(
                function.get("name"), str
            ):
                raise TypeError(
                    "function tools require a function object with a string name"
                )
            if function["name"] == self.config.retrieve_tool_name:
                if retrieve_seen or candidate != retrieve_schema:
                    raise ValueError(
                        f"Tool name {self.config.retrieve_tool_name!r} is reserved "
                        "for Tokenless retrieval"
                    )
                retrieve_seen = True
                continue
            if self.config.schema_compression_enabled:
                candidate = await self._compress_schema(candidate, request.attribution)
            tools.append(candidate)

        marker_text = request.visible_context + json.dumps(tools, ensure_ascii=False)
        markers = self.extract_markers(marker_text)
        if markers:
            tools.append(retrieve_schema)
        return replace(request, tools=tuple(tools), visible_markers=markers)

    async def before_tool_call(self, call: ToolCall) -> ToolCall:
        """Rewrite an explicitly identified shell-command argument with RTK."""
        arguments = copy.deepcopy(call.arguments)
        if not self.config.rtk_enabled or call.name == self.config.retrieve_tool_name:
            return replace(call, arguments=arguments)
        if call.command_field is None:
            return replace(call, arguments=arguments)
        command = arguments.get(call.command_field)
        if not isinstance(command, str):
            raise TypeError(
                f"command field {call.command_field!r} must contain a string"
            )
        if self._rtk_path is None:
            raise RuntimeError("Tokenless RTK path is unavailable")
        env = os.environ.copy()
        env.update(self._attribution_env(call.attribution))
        try:
            process = await asyncio.to_thread(
                subprocess.run,
                [os.fspath(self._rtk_path), "rewrite", command],
                capture_output=True,
                text=True,
                timeout=5,
                env=env,
                check=False,
            )
        except (OSError, subprocess.SubprocessError) as error:
            logger.warning("Tokenless RTK rewrite failed: %s", error)
            return replace(call, arguments=arguments)
        if process.returncode not in (0, 3):
            if process.returncode not in (1, 2):
                logger.warning(
                    "Tokenless RTK rewrite exited with %s", process.returncode
                )
            return replace(call, arguments=arguments)
        rewritten = process.stdout.strip()
        if not rewritten or rewritten == command:
            return replace(call, arguments=arguments)
        arguments[call.command_field] = self._anchor_rtk(rewritten, call.attribution)
        return replace(call, arguments=arguments, rewritten=True)

    async def after_tool_call(self, result: ToolResult) -> ToolResult:
        """Transform successful final text or annotate environment failures."""
        if result.call.name == self.config.retrieve_tool_name:
            return result
        if result.status is ToolStatus.ERROR:
            context = self._classify_environment_error(result.content)
            return replace(result, additional_context=context)
        if result.status is not ToolStatus.SUCCESS or result.call.rewritten:
            return result

        content = result.content
        if self.config.response_compression_enabled:
            compressed = await self._response.compress_text(
                content,
                tool_name=result.call.name,
                agent_id=result.call.attribution.agent_id,
                session_id=result.call.attribution.session_id,
                tool_use_id=result.call.attribution.tool_use_id,
            )
            if compressed is not None:
                content = compressed
        if (
            self.config.toon_enabled
            and not self._response.is_excluded(result.call.name)
            and self._is_structured_json(content)
        ):
            content = await self._compress_toon(content, result.call.attribution)
        if content == result.content:
            return result
        return replace(result, content=content, transformed=True)

    async def retrieve(self, request: RetrieveRequest) -> str:
        """Return a byte-exact payload only for a marker visible to the model."""
        if _HASH_PATTERN.fullmatch(request.hash) is None:
            from anolisa_tokenless.tool_response import RetrievalError

            raise RetrievalError(
                "Invalid Tokenless stash hash; expected exactly 24 hexadecimal characters."
            )
        normalized = request.hash.lower()
        if normalized not in {marker.lower() for marker in request.visible_markers}:
            from anolisa_tokenless.tool_response import RetrievalError

            raise RetrievalError(
                "The requested Tokenless marker is not visible in the current model context."
            )
        try:
            return await asyncio.to_thread(self.runtime.retrieve, normalized)
        except TokenlessError as error:
            from anolisa_tokenless.tool_response import RetrievalError

            raise RetrievalError(str(error)) from error

    async def _compress_schema(
        self, tool: dict[str, Any], attribution: Attribution
    ) -> dict[str, Any]:
        text = json.dumps(tool, ensure_ascii=False, separators=(",", ":"))
        try:
            result = await asyncio.to_thread(
                self.runtime.compress_schema,
                text,
                agent_id=attribution.agent_id,
                session_id=attribution.session_id,
                tool_use_id=attribution.tool_use_id,
            )
            if result.applied:
                value = json.loads(result.output)
                if isinstance(value, dict):
                    return value
        except (TokenlessError, ValueError, RecursionError) as error:
            logger.warning("Tokenless schema compression failed: %s", error)
        return tool

    async def _compress_toon(self, text: str, attribution: Attribution) -> str:
        try:
            result = await asyncio.to_thread(
                self.runtime.compress_toon,
                text,
                agent_id=attribution.agent_id,
                session_id=attribution.session_id,
                tool_use_id=attribution.tool_use_id,
            )
        except TokenlessError as error:
            logger.warning("Tokenless TOON compression failed: %s", error)
            return text
        if result.applied and len(result.output.encode()) < len(text.encode()):
            return result.output
        return text

    def _resolve_rtk(self) -> Path:
        resource = files("anolisa_tokenless").joinpath("_bin", "rtk")
        if not isinstance(resource, Path):
            raise RuntimeError(
                "anolisa-tokenless must be installed as an unpacked wheel so packaged "
                "RTK has a stable executable path"
            )
        if not resource.is_file() or not os.access(resource, os.X_OK):
            raise RuntimeError(
                "anolisa-tokenless installation is missing its executable packaged RTK"
            )
        return resource

    def _anchor_rtk(self, command: str, attribution: Attribution) -> str:
        if self._rtk_path is None:
            raise RuntimeError("Tokenless RTK path is unavailable")
        prefix = " ".join(
            ["env"]
            + [
                f"{name}={shlex.quote(value)}"
                for name, value in self._attribution_env(attribution).items()
            ]
            + [shlex.quote(os.fspath(self._rtk_path))]
        )
        output: list[str] = []
        cursor = 0
        command_start = True
        while cursor < len(command):
            char = command[cursor]
            if char in " \t\r\n":
                output.append(char)
                cursor += 1
                if char in "\r\n":
                    command_start = True
                continue
            if char == "#":
                end = command.find("\n", cursor)
                if end == -1:
                    output.append(command[cursor:])
                    break
                output.append(command[cursor:end])
                cursor = end
                continue

            pair = command[cursor : cursor + 2]
            operator = pair if pair in _SEGMENT_OPERATORS else None
            if operator is None and char in _SEGMENT_OPERATORS:
                operator = char
            if operator is not None:
                output.append(operator)
                cursor += len(operator)
                command_start = True
                continue

            start = cursor
            quote: str | None = None
            while cursor < len(command):
                char = command[cursor]
                if quote is None:
                    if char in {"'", '"', "`"}:
                        quote = char
                        cursor += 1
                    elif char == "\\" and cursor + 1 < len(command):
                        cursor += 2
                    elif char in " \t\r\n" or char in {";", "&", "|"}:
                        break
                    else:
                        cursor += 1
                elif char == "\\" and quote != "'" and cursor + 1 < len(command):
                    cursor += 2
                elif char == quote:
                    quote = None
                    cursor += 1
                else:
                    cursor += 1
            token = command[start:cursor]
            output.append(prefix if command_start and token == "rtk" else token)
            command_start = False
        return "".join(output)

    def _attribution_env(self, attribution: Attribution) -> dict[str, str]:
        values = {
            "TOKENLESS_AGENT_ID": attribution.agent_id,
            "TOKENLESS_SESSION_ID": attribution.session_id,
            "TOKENLESS_DATA_DIR": self.runtime.data_dir,
        }
        if attribution.tool_use_id is not None:
            values["TOKENLESS_TOOL_USE_ID"] = attribution.tool_use_id
        return values

    @staticmethod
    def extract_markers(text: str) -> frozenset[str]:
        """Extract normalized marker hashes from model-visible text."""
        return frozenset(match.lower() for match in _MARKER_PATTERN.findall(text))

    def retrieve_schema(self) -> dict[str, Any]:
        """Return the OpenAI Function Calling schema for marker retrieval."""
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
                        }
                    },
                    "required": ["hash"],
                    "additionalProperties": False,
                },
            },
        }

    @staticmethod
    def _is_structured_json(text: str) -> bool:
        try:
            value = json.loads(text)
        except (ValueError, RecursionError):
            return False
        return isinstance(value, (dict, list))

    @staticmethod
    def _classify_environment_error(text: str) -> str | None:
        lowered = text.lower()
        for patterns, category, hint in _ENV_ERRORS:
            if any(pattern in lowered for pattern in patterns):
                return f"[tokenless:env] {category}: {hint} Skip retry."
        return None
