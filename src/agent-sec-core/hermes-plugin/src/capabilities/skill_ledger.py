"""Skill-ledger capability for Hermes skill_view calls."""

from __future__ import annotations

import json
import logging
import os
from collections import OrderedDict
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from ..cli_runner import call_agent_sec_cli, trace_context
from .base import AgentSecCoreCapability

logger = logging.getLogger("agent-sec-core")

_TOOL_NAME = "skill_view"
_SKILL_MANIFEST = "SKILL.md"
_DEFAULT_HERMES_SKILLS_DIR = Path("~/.hermes/skills")
_POLICY_ASK = "ask"
_POLICY_DEBUG = "debug"
_POLICY_WARN = "warn"
_POLICY_BLOCK = "block"
_DEFAULT_POLICY = _POLICY_ASK
_VALID_POLICIES = frozenset({_POLICY_ASK, _POLICY_DEBUG, _POLICY_WARN, _POLICY_BLOCK})
_SKIP_DIRS = frozenset({".git", ".github", ".hub", ".archive", ".skill-meta"})
_CONTEXT_KEY_FIELDS = ("session_id", "task_id", "run_id")
_HERMES_SESSION_ENV = "HERMES_SESSION_ID"
_UNSUPPORTED_STATUS = "unsupported"
_UNSUPPORTED_HERMES_NOTICE = "暂不支持Hermes场景，请自行关注skill安全性。"


class _UnsupportedHermesSkillRoot(Exception):
    """Internal signal for unsupported Hermes skill root views."""

    def __init__(self, root: Path, reason: str):
        super().__init__(reason)
        self.root = root
        self.reason = reason


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
        self._policy = self._read_policy(config)
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
        """Run skill-ledger exposure summary before Hermes reads a skill."""
        if tool_name != _TOOL_NAME:
            return None
        if not isinstance(args, dict):
            self._diagnostic("[agent-sec-core] skill-ledger missing args, fail-open")
            return None

        root = self._resolved_skills_dir()
        try:
            skill_dir = self._resolve_skill_dir(args, root=root)
        except _UnsupportedHermesSkillRoot as exc:
            self._handle_unsupported_hermes(kwargs, exc.root, exc.reason)
            return None

        if skill_dir is None:
            self._diagnostic(
                "[agent-sec-core] skill-ledger could not resolve skill_dir, fail-open"
            )
            return None
        result = call_agent_sec_cli(
            ["skill-ledger", "show", str(skill_dir)],
            timeout=self._timeout,
            trace_context=trace_context(kwargs),
        )
        if not result.stdout.strip():
            self._diagnostic(
                "[agent-sec-core] skill-ledger empty CLI output, fail-open skill_dir=%s exit_code=%s",
                skill_dir,
                result.exit_code,
            )
            return None

        try:
            summary = json.loads(result.stdout)
        except (json.JSONDecodeError, ValueError):
            self._diagnostic(
                "[agent-sec-core] skill-ledger invalid CLI JSON, fail-open skill_dir=%s exit_code=%s",
                skill_dir,
                result.exit_code,
            )
            return None

        if not isinstance(summary, dict):
            self._diagnostic(
                "[agent-sec-core] skill-ledger CLI JSON is not an object, fail-open skill_dir=%s",
                skill_dir,
            )
            return None

        message = summary.get("message")
        if not isinstance(message, str) or not message.strip():
            return None

        status = str(summary.get("latestStatus", "unknown"))
        skill_name = str(summary.get("skillName") or skill_dir.name)
        message = f"Skill '{skill_name}': {message}"
        if self._policy == _POLICY_DEBUG:
            logger.debug("[agent-sec-core] skill-ledger %s", message)
            return None

        logger.warning("[agent-sec-core] skill-ledger %s", message)

        if self._policy == _POLICY_BLOCK:
            return {"action": "block", "message": message}

        self._remember_warning(kwargs, skill_name, skill_dir, status, message)
        return None

    def _on_transform_llm_output(
        self,
        response_text: str = "",
        session_id: str = "",
        **kwargs,
    ):
        """Prepend user-visible skill-ledger warnings to the final response."""
        if self._policy not in {_POLICY_ASK, _POLICY_WARN}:
            return None
        if self._max_warnings_per_turn == 0:
            return None
        if not isinstance(response_text, str) or not response_text:
            return None

        warnings = self._pop_warnings({"session_id": session_id, **kwargs})
        if not warnings:
            return None

        unsupported_warnings = [
            warning for warning in warnings if warning.status == _UNSUPPORTED_STATUS
        ]
        warnings = [
            warning for warning in warnings if warning.status != _UNSUPPORTED_STATUS
        ]

        if unsupported_warnings and not warnings:
            return f"{_UNSUPPORTED_HERMES_NOTICE}\n\n{response_text}"

        lines = [
            "[agent-sec-core skill-ledger warning]",
            "The following Hermes skills did not pass Skill Ledger checks:",
        ]
        if unsupported_warnings:
            lines.insert(0, "")
            lines.insert(0, _UNSUPPORTED_HERMES_NOTICE)
        for warning in warnings[: self._max_warnings_per_turn]:
            lines.append(
                f"- {warning.skill_name}: status={warning.status}; {warning.message}"
            )
        if len(warnings) > self._max_warnings_per_turn:
            lines.append(
                f"- ... {len(warnings) - self._max_warnings_per_turn} more warning(s)"
            )
        lines.append("")
        lines.append(response_text)
        return "\n".join(lines)

    def _resolve_skill_dir(
        self, args: dict[str, Any], *, root: Path | None = None
    ) -> Path | None:
        """Resolve a Hermes skill_view call to a local skill directory."""
        skill_name = self._extract_string(args, "name", "skill", "skill_name")
        if not skill_name:
            return None
        return self._resolve_skill_dir_from_name(skill_name, root=root)

    def _resolve_skill_dir_from_name(
        self, skill_name: str, *, root: Path | None = None
    ) -> Path | None:
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

        if root is None:
            root = self._resolved_skills_dir()
        if root is None:
            return None
        try:
            if not root.is_dir():
                return None
        except OSError as exc:
            raise _UnsupportedHermesSkillRoot(root, str(exc)) from exc
        try:
            resolved_root = root.resolve()
        except (OSError, ValueError) as exc:
            raise _UnsupportedHermesSkillRoot(root, str(exc)) from exc

        candidates: list[Path] = []
        seen: set[Path] = set()

        def record(skill_dir: Path, skill_file: Path) -> None:
            try:
                resolved_file = skill_file.resolve()
            except OSError as exc:
                raise _UnsupportedHermesSkillRoot(root, str(exc)) from exc
            except ValueError:
                return
            if not self._is_under_root(resolved_file, resolved_root):
                return
            if resolved_file in seen:
                return
            seen.add(resolved_file)
            candidates.append(skill_dir)

        relative_name = self._safe_relative_name(wanted)
        if relative_name is not None:
            direct_path = root / relative_name
            if "/" in wanted:
                try:
                    resolved_direct_path = direct_path.resolve(strict=False)
                except (OSError, ValueError) as exc:
                    raise _UnsupportedHermesSkillRoot(root, str(exc)) from exc
                if self._is_ignored_path(direct_path, root):
                    return None
                if not self._is_under_root(resolved_direct_path, resolved_root):
                    return None
                return direct_path
            direct_skill_file = direct_path / _SKILL_MANIFEST
            try:
                is_direct_skill = direct_path.is_dir() and direct_skill_file.is_file()
            except OSError as exc:
                raise _UnsupportedHermesSkillRoot(root, str(exc)) from exc
            if is_direct_skill:
                record(direct_path, direct_skill_file)

        if "/" not in wanted:
            for skill_file in self._iter_skill_files(root):
                if skill_file.parent.name == wanted:
                    record(skill_file.parent, skill_file)

        if len(candidates) > 1:
            self._diagnostic(
                "[agent-sec-core] skill-ledger ambiguous Hermes skill name=%s matches=%s, fail-open",
                wanted,
                [str(path) for path in candidates],
            )
            return None
        return candidates[0] if candidates else None

    def _resolved_skills_dir(self) -> Path | None:
        try:
            expanded = self._skills_dir.expanduser()
            return Path(os.path.abspath(os.path.normpath(os.fspath(expanded))))
        except (OSError, ValueError):
            self._diagnostic(
                "[agent-sec-core] skill-ledger invalid Hermes skills dir: %s",
                self._skills_dir,
            )
            return None

    def _iter_skill_files(self, root: Path):
        """Yield SKILL.md files under the default Hermes local skills dir."""
        try:
            skill_files = sorted(root.rglob(_SKILL_MANIFEST))
        except OSError as exc:
            raise _UnsupportedHermesSkillRoot(root, str(exc)) from exc

        for skill_file in skill_files:
            if self._is_ignored_path(skill_file, root):
                continue
            yield skill_file

    def _handle_unsupported_hermes(
        self, kwargs: dict[str, Any], root: Path, reason: str
    ) -> None:
        log_message = "[agent-sec-core] skill-ledger %s root=%s reason=%s"
        if self._policy == _POLICY_DEBUG:
            logger.debug(log_message, _UNSUPPORTED_HERMES_NOTICE, root, reason)
            return

        logger.warning(log_message, _UNSUPPORTED_HERMES_NOTICE, root, reason)
        if self._policy not in {_POLICY_ASK, _POLICY_WARN}:
            return
        self._remember_warning(
            kwargs,
            "Hermes",
            root,
            _UNSUPPORTED_STATUS,
            _UNSUPPORTED_HERMES_NOTICE,
        )

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

    @staticmethod
    def _read_policy(config: dict) -> str:
        raw_policy = config.get("policy")
        if isinstance(raw_policy, str) and raw_policy.strip():
            policy = raw_policy.strip().lower()
            if policy in _VALID_POLICIES:
                return policy
            logger.debug(
                "[agent-sec-core] skill-ledger invalid policy=%r; using %s",
                raw_policy,
                _DEFAULT_POLICY,
            )
            return _DEFAULT_POLICY

        if "enable_block" in config:
            return _POLICY_BLOCK if bool(config.get("enable_block")) else _POLICY_WARN

        return _DEFAULT_POLICY

    def _diagnostic(self, message: str, *args: Any) -> None:
        if self._policy == _POLICY_DEBUG:
            logger.debug(message, *args)
        else:
            logger.warning(message, *args)

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
        runtime_session_id = SkillLedgerCapability._runtime_session_id()
        if runtime_session_id is not None:
            return f"session_id:{runtime_session_id}"

        for field in _CONTEXT_KEY_FIELDS:
            value = kwargs.get(field)
            if isinstance(value, str) and value.strip():
                return f"{field}:{value}"
        return None

    @staticmethod
    def _runtime_session_id() -> str | None:
        try:
            from gateway.session_context import get_session_env
        except Exception:
            return None

        try:
            value = get_session_env(_HERMES_SESSION_ENV, "")
        except Exception:
            return None
        if isinstance(value, str) and value.strip():
            return value.strip()
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
