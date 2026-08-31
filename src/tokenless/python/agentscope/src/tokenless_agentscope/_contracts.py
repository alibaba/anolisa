"""Explicit AgentScope tool contracts for Tokenless lifecycle translation."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass

from anolisa_tokenless import ContentOrigin


@dataclass(frozen=True)
class ToolContract:
    """Declares the result origin and optional RTK command field of one tool."""

    content_origin: ContentOrigin
    command_field: str | None = None

    def __post_init__(self) -> None:
        object.__setattr__(self, "content_origin", ContentOrigin(self.content_origin))
        if self.command_field == "":
            raise ValueError("command_field must not be empty")
        if (
            self.command_field is not None
            and self.content_origin is not ContentOrigin.COMMAND_OUTPUT
        ):
            raise ValueError("command_field requires command_output content origin")


_COMMAND_TOOLS = frozenset(
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
    }
)
_FILE_TOOLS = frozenset(
    {
        "Read",
        "read",
        "read_file",
        "read_many_files",
        "NotebookRead",
        "notebook_read",
        "notebookread",
    }
)
_API_TOOLS = frozenset(
    {
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
    }
)


def build_tool_contracts(
    overrides: Mapping[str, ToolContract] | None,
) -> dict[str, ToolContract]:
    """Returns built-in contracts merged with explicit application contracts."""
    contracts = {
        **{
            name: ToolContract(ContentOrigin.COMMAND_OUTPUT, "command")
            for name in _COMMAND_TOOLS
        },
        **{name: ToolContract(ContentOrigin.FILE_CONTENT) for name in _FILE_TOOLS},
        **{name: ToolContract(ContentOrigin.API_RESPONSE) for name in _API_TOOLS},
    }
    for name, contract in (overrides or {}).items():
        if not name:
            raise ValueError("tool contract name must not be empty")
        if not isinstance(contract, ToolContract):
            raise TypeError("tool contract values must be ToolContract instances")
        contracts[name] = contract
    return contracts
