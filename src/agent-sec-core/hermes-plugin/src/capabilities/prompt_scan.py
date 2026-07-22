"""Prompt-scan capability — scans user input for prompt injection / jailbreak via agent-sec-cli."""

import json
import logging
import os
import time
from dataclasses import dataclass, field
from typing import Any, Callable

from ..cli_runner import call_agent_sec_cli, trace_context
from .base import AgentSecCoreCapability

logger = logging.getLogger("agent-sec-core")

_DEFAULT_WARNING_TTL_SECONDS = 300.0
_RAW_SCAN_MODE = os.environ.get("PROMPT_SCANNER_SCAN_MODE", "standard").strip().lower()
_SCAN_MODE = _RAW_SCAN_MODE
if _SCAN_MODE not in {"fast", "standard", "strict"}:
    _SCAN_MODE = "standard"
logger.info(
    "Prompt scanner scan mode: raw=%r, effective=%r", _RAW_SCAN_MODE, _SCAN_MODE
)
_USER_INPUT_SOURCE = "user_input"


@dataclass
class WarningBucket:
    """Cached warnings for a single Hermes run/session key."""

    warnings: list[str] = field(default_factory=list)
    last_touched_at: float = field(default_factory=time.monotonic)


class PromptScanCapability(AgentSecCoreCapability):
    """Scan user input for prompt injection / jailbreak attempts (non-blocking, fail-open)."""

    id = "prompt-scan-user-input"
    name = "Prompt Scanner"

    # ------------------------------------------------------------------
    # Lifecycle & registration
    # ------------------------------------------------------------------

    def __init__(self) -> None:
        super().__init__()
        self._warning_ttl_seconds: float = _DEFAULT_WARNING_TTL_SECONDS
        self._warnings_by_key: dict[str, WarningBucket] = {}

    def _on_register(self, config: dict[str, Any]) -> None:
        """Read prompt-scan specific config."""
        ttl = config.get("warning_ttl_seconds", _DEFAULT_WARNING_TTL_SECONDS)
        try:
            parsed_ttl = float(ttl)
        except (TypeError, ValueError):
            parsed_ttl = _DEFAULT_WARNING_TTL_SECONDS
        self._warning_ttl_seconds = max(0.0, parsed_ttl)

    def get_hooks_define(self) -> dict[str, Callable[..., Any]]:
        return {
            "pre_llm_call": self._on_pre_llm_call,
            "transform_llm_output": self._on_transform_llm_output,
            "on_session_end": self._on_session_end,
        }

    # ------------------------------------------------------------------
    # Hook handlers
    # ------------------------------------------------------------------

    def _on_pre_llm_call(self, messages: Any = None, **kwargs: Any) -> None:
        """Scan the current user input before the LLM turn starts."""
        self._cleanup_expired()

        user_text = self._extract_user_text(messages, kwargs)
        if not user_text.strip():
            return None

        cache_key = self._cache_key(kwargs)
        if cache_key is None:
            logger.warning(
                f"[agent-sec-core] {self.id} missing session/task key, fail-open"
            )
            return None

        # Drop any stale warning carried over from a previous turn under the
        # same correlation key — only the freshest scan should win.
        self._warnings_by_key.pop(cache_key, None)
        scan = self._scan_text(user_text, trace_context(kwargs))
        if scan is None:
            return None

        verdict = self._safe_string(scan.get("verdict")) or "pass"

        if verdict == "pass":
            logger.info(f"[agent-sec-core] {self.id} PASS")
            return None

        if verdict == "error":
            logger.warning(
                f"[agent-sec-core] {self.id} agent-sec-cli returned verdict=error, fail-open"
            )
            return None

        if verdict not in {"warn", "deny"}:
            logger.warning(
                f"[agent-sec-core] {self.id} UNKNOWN verdict={verdict}, fail-open"
            )
            return None

        warning = self._format_prompt_warning(verdict, scan)

        # Non-blocking delivery: cache warning for transform_llm_output.
        self._push_warning(cache_key, warning)
        logger.warning(
            f"[agent-sec-core] {self.id} {verdict.upper()} warning cached key={cache_key}"
        )
        return None

    def _on_transform_llm_output(
        self,
        response_text: str = "",
        session_id: str = "",
        **kwargs: Any,
    ) -> str | None:
        """Prepend cached prompt-scan warnings to the final user-visible response."""
        self._cleanup_expired()
        if not isinstance(response_text, str) or not response_text:
            return None

        cache_key = self._cache_key({"session_id": session_id, **kwargs})
        if cache_key is None:
            return None

        warnings = self._pop_warnings(cache_key)
        if not warnings:
            return None

        return "\n".join(warnings) + "\n\n" + response_text

    def _on_session_end(self, session_id: str = "", **kwargs: Any) -> None:
        """Clean cached warnings when Hermes ends a session."""
        cache_key = self._cache_key({"session_id": session_id, **kwargs})
        if cache_key is not None:
            self._warnings_by_key.pop(cache_key, None)
        self._cleanup_expired()
        return None

    # ------------------------------------------------------------------
    # CLI invocation
    # ------------------------------------------------------------------

    def _scan_text(
        self,
        text: str,
        security_trace_context: dict[str, str] | None,
    ) -> dict[str, Any] | None:
        """Run agent-sec-cli scan-prompt and parse its JSON output.

        The prompt text is piped via stdin instead of being passed as an
        ``--text`` argv to avoid two issues:
        1. ARG_MAX (~2MB on Linux) — large RAG-injected / multi-turn prompts
           would trigger E2BIG and silently fail-open.
        2. ``ps aux`` / ``/proc/<pid>/cmdline`` leakage — argv is world-readable
           on the same host while the subprocess is alive.
        """
        args = [
            "scan-prompt",
            "--mode",
            _SCAN_MODE,
            "--format",
            "json",
            "--source",
            _USER_INPUT_SOURCE,
        ]

        result = call_agent_sec_cli(
            args,
            timeout=self._timeout,
            stdin=text,
            trace_context=security_trace_context,
        )
        if result.exit_code != 0:
            logger.warning(
                f"[agent-sec-core] {self.id} agent-sec-cli exit_code={result.exit_code}, fail-open"
            )
            return None

        try:
            scan = json.loads(result.stdout)
        except (json.JSONDecodeError, ValueError):
            logger.warning(
                f"[agent-sec-core] {self.id} agent-sec-cli returned invalid JSON, fail-open"
            )
            return None

        if not isinstance(scan, dict):
            logger.warning(
                f"[agent-sec-core] {self.id} agent-sec-cli returned non-object JSON, fail-open"
            )
            return None
        return scan

    # ------------------------------------------------------------------
    # Input extraction helpers
    # ------------------------------------------------------------------

    def _extract_user_text(self, messages: Any, kwargs: dict[str, Any]) -> str:
        """Extract only the current user input from Hermes hook payloads."""
        for key in ("user_message", "user_input", "prompt"):
            value = kwargs.get(key)
            if isinstance(value, str) and value.strip():
                return value

        if not isinstance(messages, list):
            return ""

        for message in reversed(messages):
            role = self._message_value(message, "role")
            if role != "user":
                continue
            return self._content_to_text(self._message_value(message, "content"))
        return ""

    def _content_to_text(self, content: Any) -> str:
        """Convert common message content shapes to text."""
        if isinstance(content, str):
            return content
        if isinstance(content, list):
            parts: list[str] = []
            for item in content:
                if isinstance(item, str):
                    parts.append(item)
                    continue
                text = self._message_value(item, "text")
                if isinstance(text, str):
                    parts.append(text)
            return "\n".join(parts)
        return ""

    # ------------------------------------------------------------------
    # Warning cache helpers
    # ------------------------------------------------------------------

    def _cache_key(self, values: dict[str, Any]) -> str | None:
        """Return the best available Hermes turn/session correlation key."""
        for key in ("session_id", "task_id", "run_id"):
            value = values.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
        return None

    def _push_warning(self, cache_key: str, warning: str) -> None:
        """Cache a warning for later transform_llm_output delivery."""
        self._cleanup_expired()
        now = time.monotonic()
        bucket = self._warnings_by_key.get(cache_key)
        if bucket is None:
            bucket = WarningBucket(last_touched_at=now)
        if warning not in bucket.warnings:
            bucket.warnings.append(warning)
        bucket.last_touched_at = now
        self._warnings_by_key[cache_key] = bucket

    def _pop_warnings(self, cache_key: str) -> list[str]:
        """Return and remove cached warnings for a key."""
        bucket = self._warnings_by_key.pop(cache_key, None)
        if bucket is None:
            return []
        return list(bucket.warnings)

    def _cleanup_expired(self) -> None:
        """Remove stale warning buckets."""
        ttl = self._warning_ttl_seconds
        now = time.monotonic()
        expired = [
            cache_key
            for cache_key, bucket in self._warnings_by_key.items()
            if now - bucket.last_touched_at >= ttl
        ]
        for cache_key in expired:
            self._warnings_by_key.pop(cache_key, None)

    # ------------------------------------------------------------------
    # Formatting & misc helpers
    # ------------------------------------------------------------------

    def _format_prompt_warning(self, verdict: str, scan: dict[str, Any]) -> str:
        """Build a warning string from a scan-prompt result."""
        threat_type = self._safe_string(scan.get("threat_type"))
        risk_level = self._safe_string(scan.get("risk_level")) or "unknown"
        confidence = scan.get("confidence")

        lines = [
            "\U0001f6e1\ufe0f [prompt-scan] 检测到安全风险",
            f"  攻击类型 : {threat_type or 'unknown'}",
            f"  风险等级 : {risk_level}",
            "  拦截环节 : 用户输入扫描 (pre_llm_call)",
        ]
        if confidence is not None:
            try:
                lines.append(f"  模型置信度: {float(confidence) * 100:.1f}%")
            except (TypeError, ValueError):
                pass
        lines.append("")
        lines.append("本轮请求将继续处理。")
        return "\n".join(lines)

    def _message_value(self, message: Any, key: str) -> Any:
        """Read a key from dict-like or object-like messages."""
        if isinstance(message, dict):
            return message.get(key)
        return getattr(message, key, None)

    def _safe_string(self, value: Any) -> str:
        return value if isinstance(value, str) else ""
