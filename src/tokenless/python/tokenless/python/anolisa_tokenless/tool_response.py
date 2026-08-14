"""Framework-neutral policy for reversible tool-response compression."""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
from collections.abc import Collection
from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import Path
from typing import Any

from anolisa_tokenless._native import TokenlessError, TokenlessRuntime

logger = logging.getLogger(__name__)

_HASH_PATTERN = re.compile(r"^[0-9a-fA-F]{24}$")

# Keep these names aligned with common/hooks/tool_categories.json. Framework
# packages cannot load that sibling resource after independent installation, so
# a repository test guards this policy against drift.
SKIP_TOOLS = frozenset(
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
SHELL_TOOLS = frozenset(
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

CONSERVATIVE_THRESHOLDS = (1_048_576, 65_536, 32)
SHELL_THRESHOLDS = (65_536, 128, 8)
AGGRESSIVE_THRESHOLDS = (4_096, 32, 8)


class CompressionMode(StrEnum):
    """Supported tool-response compression policies."""

    CONSERVATIVE = "conservative"
    BALANCED = "balanced"
    AGGRESSIVE = "aggressive"


class RetrievalError(ValueError):
    """Raised when a stash retrieval request is invalid or unauthorized."""


@dataclass(frozen=True)
class TokenlessConfig:
    """Configuration shared by framework integrations."""

    mode: CompressionMode | str = CompressionMode.BALANCED
    data_dir: str | os.PathLike[str] | None = None
    min_chars: int = 200
    excluded_tools: Collection[str] = field(default_factory=tuple)
    retrieve_tool_name: str = "tokenless_retrieve"

    def __post_init__(self) -> None:
        """Normalize configuration at the framework boundary."""
        object.__setattr__(self, "mode", CompressionMode(self.mode))
        if self.min_chars < 0:
            raise ValueError("min_chars must be non-negative")
        if not self.retrieve_tool_name:
            raise ValueError("retrieve_tool_name must not be empty")

        if self.data_dir is not None:
            data_dir = Path(self.data_dir).expanduser()
            if not data_dir.is_absolute():
                raise ValueError("data_dir must be an absolute path")
            object.__setattr__(self, "data_dir", os.fspath(data_dir))
        object.__setattr__(
            self,
            "excluded_tools",
            frozenset(self.excluded_tools) | {self.retrieve_tool_name},
        )


class ToolResponseCompressor:
    """Apply common Tokenless policy without depending on an agent framework."""

    def __init__(self, config: TokenlessConfig) -> None:
        """Create a compressor and its tenant-scoped runtime."""
        self.config = config
        self.runtime = TokenlessRuntime(config.data_dir)

    def is_excluded(self, tool_name: str) -> bool:
        """Return whether policy excludes a tool from response compression."""
        if tool_name in self.config.excluded_tools:
            return True
        return (
            self.config.mode is not CompressionMode.CONSERVATIVE
            and tool_name in SKIP_TOOLS
        )

    def thresholds_for(self, tool_name: str) -> tuple[int, int, int]:
        """Return truncation thresholds for a tool under the configured mode."""
        if self.config.mode is CompressionMode.AGGRESSIVE:
            return AGGRESSIVE_THRESHOLDS
        if self.config.mode is CompressionMode.BALANCED and tool_name in SHELL_TOOLS:
            return SHELL_THRESHOLDS
        return CONSERVATIVE_THRESHOLDS

    async def compress_text(
        self,
        text: str,
        *,
        tool_name: str,
        agent_id: str,
        session_id: str | None,
        tool_use_id: str | None,
    ) -> str | None:
        """Compress text in a worker thread, preserving type and failing open."""
        if self.is_excluded(tool_name) or len(text) < self.config.min_chars:
            return None
        input_text, expected_type = self._normalize_input(text)
        thresholds = self.thresholds_for(tool_name)
        try:
            result = await asyncio.to_thread(
                self.runtime.compress_response,
                input_text,
                truncate_strings_at=thresholds[0],
                truncate_arrays_at=thresholds[1],
                max_depth=thresholds[2],
                agent_id=agent_id,
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
        except (ValueError, RecursionError):
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

    async def retrieve(self, hash_value: str, visible_context: str) -> str:
        """Retrieve only a payload whose marker is visible to the agent."""
        if (
            not isinstance(hash_value, str)
            or _HASH_PATTERN.fullmatch(hash_value) is None
        ):
            raise RetrievalError(
                "Invalid Tokenless stash hash; expected exactly 24 hexadecimal characters.",
            )

        normalized = hash_value.lower()
        marker = f"<<tokenless:{normalized}>>"
        if marker not in visible_context.lower():
            raise RetrievalError(
                "The requested Tokenless marker is not present in the current session context.",
            )

        try:
            return await asyncio.to_thread(self.runtime.retrieve, normalized)
        except TokenlessError as error:
            raise RetrievalError(str(error)) from error

    @staticmethod
    def _normalize_input(text: str) -> tuple[str, type[Any]]:
        try:
            value = json.loads(text)
        except (ValueError, RecursionError):
            value = None
        if isinstance(value, (dict, list)):
            return text, type(value)
        return json.dumps(text, ensure_ascii=False), str
