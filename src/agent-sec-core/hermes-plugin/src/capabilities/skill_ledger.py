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
_DEFAULT_HERMES_SKILLS_DIR = Path("~/.hermes/skills")
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
        self._skills_dir = _DEFAULT_HERMES_SKILLS_DIR
        self._max_warnings_per_turn = self._read_int_config(
            config, "max_warnings_per_turn", default=5, minimum=0
        )
        self._max_warning_contexts = self._read_int_config(
            config, "max_warning_contexts", default=128, minimum=1
        )

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

        skill_dir = self._resolve_skill_dir(args)
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
        if self._max_warnings_per_turn == 0:
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

    def _resolve_skill_dir(self, args: dict[str, Any]) -> Path | None:
        """Resolve a Hermes skill_view call to a local skill directory."""
        skill_name = self._extract_string(args, "name", "skill", "skill_name")
        if not skill_name:
            return None
        return self._resolve_skill_dir_from_name(skill_name)

    def _resolve_skill_dir_from_name(self, skill_name: str) -> Path | None:
        """Resolve by Hermes local directory name or category/name."""
        wanted = skill_name.strip()
        if not wanted:
            return None
        if ":" in wanted:
            logger.debug(
                "[agent-sec-core] skill-ledger skips qualified/plugin skill name: %s",
                wanted,
            )
            return None

        root = self._resolved_skills_dir()
        if root is None or not root.is_dir():
            return None

        candidates: list[Path] = []
        seen: set[Path] = set()

        def record(skill_dir: Path, skill_file: Path) -> None:
            try:
                resolved_file = skill_file.resolve()
                resolved_dir = skill_dir.resolve()
            except (OSError, ValueError):
                return
            if not self._is_under_root(resolved_file, root):
                return
            if resolved_file in seen:
                return
            seen.add(resolved_file)
            candidates.append(resolved_dir)

        relative_name = self._safe_relative_name(wanted)
        if relative_name is not None:
            direct_path = root / relative_name
            direct_skill_file = direct_path / _SKILL_MANIFEST
            if direct_path.is_dir() and direct_skill_file.is_file():
                record(direct_path, direct_skill_file)

        if "/" not in wanted:
            for skill_file in self._iter_skill_files(root):
                if skill_file.parent.name == wanted:
                    record(skill_file.parent, skill_file)

        if len(candidates) > 1:
            logger.warning(
                "[agent-sec-core] skill-ledger ambiguous Hermes skill name=%s matches=%s, fail-open",
                wanted,
                [str(path) for path in candidates],
            )
            return None
        return candidates[0] if candidates else None

    def _resolved_skills_dir(self) -> Path | None:
        try:
            return self._skills_dir.expanduser().resolve()
        except (OSError, ValueError):
            logger.warning(
                "[agent-sec-core] skill-ledger invalid Hermes skills dir: %s",
                self._skills_dir,
            )
            return None

    def _iter_skill_files(self, root: Path):
        """Yield SKILL.md files under the default Hermes local skills dir."""
        for skill_file in sorted(root.rglob(_SKILL_MANIFEST)):
            try:
                resolved = skill_file.resolve()
            except (OSError, ValueError):
                continue
            if self._is_ignored_path(resolved, root):
                continue
            yield resolved

    @staticmethod
    def _is_ignored_path(path: Path, root: Path) -> bool:
        try:
            parts = path.relative_to(root).parts
        except ValueError:
            return True
        return any(part in _SKIP_DIRS for part in parts)

    @staticmethod
    def _is_under_root(path: Path, root: Path) -> bool:
        try:
            path.relative_to(root)
        except ValueError:
            return False
        return True

    @staticmethod
    def _safe_relative_name(skill_name: str) -> Path | None:
        path = Path(skill_name)
        if path.is_absolute() or ".." in path.parts:
            return None
        return path

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
        if self._max_warnings_per_turn == 0:
            return
        context_key = self._context_key(kwargs)
        if context_key is None:
            logger.debug(
                "[agent-sec-core] skill-ledger warning has no stable context; user-visible injection skipped"
            )
            return
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
        if context_key is None:
            return []
        if context_key in self._warnings_by_context:
            return list(self._warnings_by_context.pop(context_key).values())
        return []

    @staticmethod
    def _context_key(kwargs: dict[str, Any]) -> str | None:
        for field in _CONTEXT_KEY_FIELDS:
            value = kwargs.get(field)
            if isinstance(value, str) and value.strip():
                return f"{field}:{value}"
        return None

    @staticmethod
    def _read_int_config(config: dict, key: str, *, default: int, minimum: int) -> int:
        raw = config.get(key, default)
        try:
            value = int(raw)
        except (TypeError, ValueError):
            logger.warning(
                "[agent-sec-core] skill-ledger invalid integer config %s=%r; using %s",
                key,
                raw,
                default,
            )
            return default
        if value < minimum:
            logger.warning(
                "[agent-sec-core] skill-ledger config %s=%r below minimum %s; using %s",
                key,
                raw,
                minimum,
                minimum,
            )
            return minimum
        return value

    @staticmethod
    def _format_message(status: str, skill_name: str, skill_dir: Path) -> str:
        detail = _STATUS_MESSAGES.get(status, f"Skill has unknown status '{status}'.")
        return f"Skill '{skill_name}' ({skill_dir}) status={status}. {detail}"
