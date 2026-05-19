"""Skill-ledger capability for Hermes skill_view calls."""

from __future__ import annotations

import json
import logging
from collections import OrderedDict
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ..cli_runner import call_agent_sec_cli
from .base import AgentSecCoreCapability

logger = logging.getLogger("agent-sec-core")

_TOOL_NAME = "skill_view"
_SKILL_MANIFEST = "SKILL.md"
_DEFAULT_SKILL_ROOTS = ["~/.hermes/skills"]
_DEFAULT_BLOCK_STATUSES = ["none", "drifted", "deny", "tampered"]
_SKIP_DIRS = frozenset({".git", ".github", ".hub", ".archive", ".skill-meta"})
_CONTEXT_KEY_FIELDS = ("session_id", "task_id", "run_id", "conversation_id")

_STATUS_MESSAGES = {
    "none": "Skill has not been security-scanned yet.",
    "warn": "Skill has low-risk findings; review is recommended.",
    "drifted": "Skill content changed since the last scan.",
    "deny": "Skill has high-risk findings.",
    "tampered": "Skill metadata signature verification failed.",
    "error": "Skill check failed.",
}


@dataclass
class SkillWarning:
    """User-visible warning captured during pre_tool_call."""

    skill_name: str
    skill_dir: str
    status: str
    message: str


class SkillLedgerCapability(AgentSecCoreCapability):
    """Check Hermes skills with skill-ledger before skill_view reads them."""

    id = "skill-ledger"
    name = "Skill Ledger"

    def __init__(self):
        super().__init__()
        self._warnings_by_context: OrderedDict[str, dict[str, SkillWarning]] = (
            OrderedDict()
        )

    def _on_register(self, config: dict) -> None:
        """Read skill-ledger specific config."""
        self._enable_block = bool(config.get("enable_block", False))
        statuses = config.get("block_statuses", _DEFAULT_BLOCK_STATUSES)
        if not isinstance(statuses, list):
            statuses = _DEFAULT_BLOCK_STATUSES
        self._block_statuses = {str(s) for s in statuses}
        roots = config.get("skill_roots", _DEFAULT_SKILL_ROOTS)
        if not isinstance(roots, list):
            roots = _DEFAULT_SKILL_ROOTS
        self._skill_roots = [str(root) for root in roots if str(root).strip()]
        max_warnings = config.get("max_warnings_per_turn", 5)
        self._max_warnings_per_turn = max(1, int(max_warnings))
        max_contexts = config.get("max_warning_contexts", 128)
        self._max_warning_contexts = max(1, int(max_contexts))

    def get_hooks_define(self) -> dict:
        return {
            "pre_tool_call": self._on_pre_tool_call,
            "transform_llm_output": self._on_transform_llm_output,
        }

    def _on_pre_tool_call(self, tool_name, args, **kwargs):
        """Run skill-ledger check before Hermes reads a skill."""
        if tool_name != _TOOL_NAME:
            return None
        if not isinstance(args, dict):
            logger.warning("[agent-sec-core] skill-ledger missing args, fail-open")
            return None

        skill_dir = self._resolve_skill_dir(args, kwargs)
        if skill_dir is None:
            logger.warning(
                "[agent-sec-core] skill-ledger could not resolve skill_dir, fail-open"
            )
            return None
        skill_dir = skill_dir.resolve()

        result = call_agent_sec_cli(
            ["skill-ledger", "check", str(skill_dir)],
            timeout=self._timeout,
        )
        if not result.stdout.strip():
            logger.warning(
                "[agent-sec-core] skill-ledger empty CLI output, fail-open skill_dir=%s exit_code=%s",
                skill_dir,
                result.exit_code,
            )
            return None

        try:
            check_result = json.loads(result.stdout)
        except (json.JSONDecodeError, ValueError):
            logger.warning(
                "[agent-sec-core] skill-ledger invalid CLI JSON, fail-open skill_dir=%s exit_code=%s",
                skill_dir,
                result.exit_code,
            )
            return None

        if not isinstance(check_result, dict):
            logger.warning(
                "[agent-sec-core] skill-ledger CLI JSON is not an object, fail-open skill_dir=%s",
                skill_dir,
            )
            return None

        status = str(check_result.get("status", "unknown"))
        if status == "pass":
            return None

        skill_name = str(check_result.get("skillName") or skill_dir.name)
        message = self._format_message(status, skill_name, skill_dir)
        logger.warning("[agent-sec-core] skill-ledger %s", message)

        if self._enable_block:
            if status in self._block_statuses:
                return {"action": "block", "message": message}
            return None

        self._remember_warning(kwargs, skill_name, skill_dir, status, message)
        return None

    def _on_transform_llm_output(self, response=None, **kwargs):
        """Prepend user-visible skill-ledger warnings to the final response."""
        if self._enable_block:
            return None
        if not isinstance(response, str):
            return None

        warnings = self._pop_warnings(kwargs)
        if not warnings:
            return None

        lines = [
            "[agent-sec-core skill-ledger warning]",
            "The following Hermes skills did not pass Skill Ledger checks:",
        ]
        for warning in warnings[: self._max_warnings_per_turn]:
            lines.append(
                f"- {warning.skill_name}: status={warning.status}; {warning.message}"
            )
        if len(warnings) > self._max_warnings_per_turn:
            lines.append(
                f"- ... {len(warnings) - self._max_warnings_per_turn} more warning(s)"
            )
        lines.append("")
        lines.append(response)
        return "\n".join(lines)

    def _resolve_skill_dir(
        self, args: dict[str, Any], kwargs: dict[str, Any]
    ) -> Path | None:
        """Resolve a Hermes skill_view call to a local skill directory."""
        direct_path = self._extract_string(args, "file_path", "path")
        if direct_path:
            skill_dir = self._resolve_skill_dir_from_file_path(direct_path, kwargs)
            if skill_dir is not None:
                return skill_dir

        skill_name = self._extract_string(args, "name", "skill", "skill_name")
        if not skill_name:
            return None
        return self._resolve_skill_dir_from_name(skill_name)

    def _resolve_skill_dir_from_file_path(
        self, file_path: str, kwargs: dict[str, Any]
    ) -> Path | None:
        """Resolve file_path when Hermes directly points at SKILL.md."""
        path = Path(file_path).expanduser()
        if not path.is_absolute():
            cwd = kwargs.get("cwd")
            if isinstance(cwd, str) and cwd.strip():
                path = Path(cwd).expanduser() / path
            else:
                return None
        try:
            resolved = path.resolve()
        except (OSError, ValueError):
            return None
        if resolved.name != _SKILL_MANIFEST or not resolved.is_file():
            return None
        return resolved.parent

    def _resolve_skill_dir_from_name(self, skill_name: str) -> Path | None:
        """Resolve by directory name, category/name, or SKILL.md frontmatter name."""
        wanted = skill_name.strip()
        if not wanted:
            return None
        for skill_file in self._iter_skill_files():
            skill_dir = skill_file.parent
            names = {
                skill_dir.name,
                self._frontmatter_name(skill_file),
            }
            for root in self._resolved_skill_roots():
                try:
                    names.add(skill_dir.relative_to(root).as_posix())
                except ValueError:
                    continue
            if wanted in {name for name in names if name}:
                return skill_dir
        return None

    def _iter_skill_files(self):
        """Yield SKILL.md files under configured Hermes skill roots."""
        seen: set[Path] = set()
        for root in self._resolved_skill_roots():
            if not root.is_dir():
                continue
            for skill_file in sorted(root.rglob(_SKILL_MANIFEST)):
                try:
                    resolved = skill_file.resolve()
                except (OSError, ValueError):
                    continue
                if resolved in seen or self._is_ignored_path(resolved, root):
                    continue
                seen.add(resolved)
                yield resolved

    def _resolved_skill_roots(self) -> list[Path]:
        roots: list[Path] = []
        for raw_root in self._skill_roots:
            try:
                roots.append(Path(raw_root).expanduser().resolve())
            except (OSError, ValueError):
                logger.warning(
                    "[agent-sec-core] skill-ledger invalid skill root: %s", raw_root
                )
        return roots

    @staticmethod
    def _is_ignored_path(path: Path, root: Path) -> bool:
        try:
            parts = path.relative_to(root).parts
        except ValueError:
            return True
        return any(part.startswith(".") or part in _SKIP_DIRS for part in parts)

    @staticmethod
    def _frontmatter_name(skill_file: Path) -> str | None:
        try:
            text = skill_file.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            return None
        if not text.startswith("---"):
            return None
        for line in text.splitlines()[1:40]:
            if line.strip() == "---":
                return None
            if line.startswith("name:"):
                return line.split(":", 1)[1].strip().strip("\"'")
        return None

    @staticmethod
    def _extract_string(args: dict[str, Any], *keys: str) -> str | None:
        for key in keys:
            value = args.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
        return None

    def _remember_warning(
        self,
        kwargs: dict[str, Any],
        skill_name: str,
        skill_dir: Path,
        status: str,
        message: str,
    ) -> None:
        context_key = self._context_key(kwargs)
        bucket = self._warnings_by_context.setdefault(context_key, {})
        bucket[str(skill_dir)] = SkillWarning(
            skill_name=skill_name,
            skill_dir=str(skill_dir),
            status=status,
            message=message,
        )
        self._warnings_by_context.move_to_end(context_key)
        while len(self._warnings_by_context) > self._max_warning_contexts:
            self._warnings_by_context.popitem(last=False)

    def _pop_warnings(self, kwargs: dict[str, Any]) -> list[SkillWarning]:
        context_key = self._context_key(kwargs)
        if context_key in self._warnings_by_context:
            return list(self._warnings_by_context.pop(context_key).values())
        if context_key != "__global__" and "__global__" in self._warnings_by_context:
            return list(self._warnings_by_context.pop("__global__").values())
        return []

    @staticmethod
    def _context_key(kwargs: dict[str, Any]) -> str:
        for field in _CONTEXT_KEY_FIELDS:
            value = kwargs.get(field)
            if isinstance(value, str) and value.strip():
                return f"{field}:{value}"
        return "__global__"

    @staticmethod
    def _format_message(status: str, skill_name: str, skill_dir: Path) -> str:
        detail = _STATUS_MESSAGES.get(status, f"Skill has unknown status '{status}'.")
        return f"Skill '{skill_name}' ({skill_dir}) status={status}. {detail}"
