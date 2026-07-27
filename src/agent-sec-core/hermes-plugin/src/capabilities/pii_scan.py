"""PII-scan capability — scans user input via agent-sec-cli."""

from __future__ import annotations

import json
import logging
import time
from dataclasses import dataclass, field
from typing import Any

from ..cli_runner import call_agent_sec_cli, trace_context
from ..pii_text import extract_user_text, value_to_text
from .base import AgentSecCoreCapability

logger = logging.getLogger("agent-sec-core")

_DEFAULT_WARNING_TTL_SECONDS = 300.0
_MAX_EVIDENCE_ITEMS = 3
_MAX_EVIDENCE_CHARS = 80
_USER_INPUT_SOURCE = "user_input"
_TOOL_INPUT_SOURCE = "tool_input"
_TOOL_OUTPUT_SOURCE = "tool_output"
_MODEL_OUTPUT_SOURCE = "model_output"
_CONTEXT_KEY_FIELDS = ("session_id", "task_id", "run_id")
_HERMES_SESSION_ENV = "HERMES_SESSION_ID"


@dataclass
class WarningBucket:
    """Cached warnings for a single Hermes run/session key."""

    warnings: list[str] = field(default_factory=list)
    created_at: float = field(default_factory=time.monotonic)
    last_touched_at: float = field(default_factory=time.monotonic)


class PiiScanCapability(AgentSecCoreCapability):
    """Scan the current user turn for PII and show a non-blocking warning."""

    id = "pii-scan-user-input"
    name = "PII Checker"

    def __init__(self):
        super().__init__()
        self._include_low_confidence = False
        self._warning_ttl_seconds = _DEFAULT_WARNING_TTL_SECONDS
        self._warnings_by_key: dict[str, WarningBucket] = {}

    def _on_register(self, config: dict) -> None:
        """Read pii-scan specific config."""
        self._include_low_confidence = bool(config.get("include_low_confidence", False))
        ttl = config.get("warning_ttl_seconds", _DEFAULT_WARNING_TTL_SECONDS)
        try:
            parsed_ttl = float(ttl)
        except (TypeError, ValueError):
            parsed_ttl = _DEFAULT_WARNING_TTL_SECONDS
        self._warning_ttl_seconds = max(0.0, parsed_ttl)

    def get_hooks_define(self) -> dict:
        return {
            "pre_llm_call": self._on_pre_llm_call,
            "pre_tool_call": self._on_pre_tool_call,
            "post_tool_call": self._on_post_tool_call,
            "transform_llm_output": self._on_transform_llm_output,
            "on_session_end": self._on_session_end,
        }

    def _on_pre_llm_call(self, messages=None, **kwargs):
        """Scan the current user input before the LLM turn starts."""
        self._cleanup_expired()

        user_text = extract_user_text(messages, kwargs)
        if not user_text.strip():
            return None

        cache_key = self._cache_key(kwargs)
        if cache_key is None:
            logger.warning(
                f"[agent-sec-core] {self.id} missing session/task key, fail-open"
            )
            return None

        self._warnings_by_key.pop(cache_key, None)
        self._scan_and_cache(
            user_text,
            source=_USER_INPUT_SOURCE,
            cache_key=cache_key,
            security_trace_context=trace_context(kwargs),
        )
        return None

    def _on_pre_tool_call(
        self,
        *,
        tool_name: Any,
        args: Any,
        **kwargs: Any,
    ):
        """Scan tool arguments before execution."""
        self._cleanup_expired()
        text = self._value_to_text(args)
        if not text.strip():
            return None
        cache_key = self._cache_key(kwargs)
        if cache_key is None:
            logger.warning(
                f"[agent-sec-core] {self.id} missing session/task key for tool input, fail-open"
            )
            return None
        data = {"tool_name": tool_name, "args": args, **kwargs}
        self._scan_and_cache(
            text,
            source=_TOOL_INPUT_SOURCE,
            cache_key=cache_key,
            security_trace_context=trace_context(data),
        )
        return None

    def _on_post_tool_call(
        self,
        *,
        tool_name: Any,
        args: Any,
        result: Any,
        **kwargs: Any,
    ):
        """Scan tool output after execution."""
        self._cleanup_expired()
        text = self._value_to_text(result)
        if not text.strip():
            return None
        cache_key = self._cache_key(kwargs)
        if cache_key is None:
            logger.warning(
                f"[agent-sec-core] {self.id} missing session/task key for tool output, fail-open"
            )
            return None
        data = {"tool_name": tool_name, "args": args, "result": result, **kwargs}
        self._scan_and_cache(
            text,
            source=_TOOL_OUTPUT_SOURCE,
            cache_key=cache_key,
            security_trace_context=trace_context(data),
        )
        return None

    def _on_transform_llm_output(
        self,
        response_text: str = "",
        session_id: str = "",
        **kwargs,
    ):
        """Prepend cached PII warnings to the final user-visible response."""
        self._cleanup_expired()
        if not isinstance(response_text, str):
            return None

        cache_key = self._cache_key({"session_id": session_id, **kwargs})
        if cache_key is None:
            return None

        warnings = self._pop_warnings(cache_key) if cache_key is not None else []
        output_text = response_text
        if response_text.strip():
            scan = self._scan_text(
                response_text,
                source=_MODEL_OUTPUT_SOURCE,
                security_trace_context=trace_context(
                    {"session_id": session_id, **kwargs}
                ),
            )
            if scan is not None:
                verdict = self._safe_string(scan.get("verdict")) or "pass"
                findings = self._as_list(scan.get("findings"))
                if verdict in {"warn", "deny"} and findings:
                    warnings.append(self._format_pii_warning(verdict, findings))
                    redacted_text = self._safe_string(scan.get("redacted_text"))
                    if redacted_text:
                        output_text = redacted_text
                        logger.warning(
                            f"[agent-sec-core] {self.id} {verdict.upper()} model output redacted"
                        )
                    else:
                        output_text = ""
                        logger.warning(
                            f"[agent-sec-core] {self.id} {verdict.upper()} model output redaction missing redacted_text; dropping raw output"
                        )
                elif verdict not in {"pass", "warn", "deny"}:
                    logger.warning(
                        f"[agent-sec-core] {self.id} UNKNOWN model output verdict={verdict}, fail-open"
                    )

        if not warnings and output_text == response_text:
            return None

        if warnings:
            warning_text = "\n".join(warnings)
            if output_text:
                return f"{warning_text}\n\n{output_text}"
            return warning_text

        return output_text

    def _on_session_end(self, session_id: str = "", **kwargs):
        """Clean cached warnings when Hermes ends a session."""
        cache_key = self._cache_key({"session_id": session_id, **kwargs})
        if cache_key is not None:
            self._warnings_by_key.pop(cache_key, None)
        self._cleanup_expired()
        return None

    def _scan_text(
        self,
        text: str,
        *,
        source: str,
        security_trace_context: dict[str, str] | None,
    ) -> dict[str, Any] | None:
        """Run agent-sec-cli scan-pii and parse its JSON output."""
        args = [
            "scan-pii",
            "--stdin",
            "--format",
            "json",
            "--redact-output",
            "--source",
            source,
        ]
        if self._include_low_confidence:
            args.append("--include-low-confidence")

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

    def _scan_and_cache(
        self,
        text: str,
        *,
        source: str,
        cache_key: str,
        security_trace_context: dict[str, str] | None,
    ) -> None:
        """Scan text and cache a minimal warning for warn/deny results."""
        scan = self._scan_text(
            text,
            source=source,
            security_trace_context=security_trace_context,
        )
        if scan is None:
            return

        verdict = self._safe_string(scan.get("verdict")) or "pass"
        findings = self._as_list(scan.get("findings"))

        if verdict == "pass" or not findings:
            logger.info(f"[agent-sec-core] {self.id} PASS source={source}")
            return

        if verdict not in {"warn", "deny"}:
            logger.warning(
                f"[agent-sec-core] {self.id} UNKNOWN verdict={verdict}, fail-open"
            )
            return

        warning = self._format_pii_warning(verdict, findings)
        self._push_warning(cache_key, warning)
        logger.warning(
            f"[agent-sec-core] {self.id} {verdict.upper()} warning cached key={cache_key} source={source}"
        )

    def _value_to_text(self, value: Any) -> str:
        """Convert arbitrary hook values into scan text."""
        return value_to_text(value)

    def _cache_key(self, values: dict[str, Any]) -> str | None:
        """Return the best available Hermes turn/session correlation key."""
        runtime_session_id = self._runtime_session_id()
        if runtime_session_id is not None:
            return f"session_id:{runtime_session_id}"

        for key in _CONTEXT_KEY_FIELDS:
            value = values.get(key)
            if isinstance(value, str) and value.strip():
                return f"{key}:{value.strip()}"
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

    def _push_warning(self, cache_key: str, warning: str) -> None:
        """Cache a warning for later transform_llm_output delivery."""
        self._cleanup_expired()
        now = time.monotonic()
        bucket = self._warnings_by_key.get(cache_key)
        if bucket is None:
            bucket = WarningBucket(created_at=now, last_touched_at=now)
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

    def _format_pii_warning(self, verdict: str, findings: list[Any]) -> str:
        """Build a minimal-disclosure warning from structured PII findings."""
        typed_findings = [item for item in findings if isinstance(item, dict)]
        pii_types = sorted(
            {
                finding_type
                for finding in typed_findings
                if (finding_type := self._safe_string(finding.get("type")))
            }
        )
        severities = sorted(
            {
                severity
                for finding in typed_findings
                if (severity := self._safe_string(finding.get("severity")))
            }
        )
        redacted_evidence: list[str] = []
        for finding in typed_findings:
            evidence = self._safe_string(finding.get("evidence_redacted"))
            if evidence and evidence not in redacted_evidence:
                redacted_evidence.append(self._shorten(evidence))
            if len(redacted_evidence) >= _MAX_EVIDENCE_ITEMS:
                break

        risk = "高风险敏感信息" if verdict == "deny" else "敏感信息"
        parts = [
            f"[pii-checker] 检测到 {len(typed_findings)} 项{risk}",
            f"类型：{', '.join(pii_types) if pii_types else 'unknown'}",
        ]
        if severities:
            parts.append(f"严重级别：{', '.join(severities)}")
        if redacted_evidence:
            parts.append(f"脱敏示例：{', '.join(redacted_evidence)}")
        parts.append("本轮请求将继续处理。")
        return "；".join(parts)

    def _shorten(self, value: str, limit: int = _MAX_EVIDENCE_CHARS) -> str:
        """Shorten evidence for display."""
        normalized = " ".join(value.split())
        if len(normalized) <= limit:
            return normalized
        return normalized[: limit - 1] + "…"

    def _as_list(self, value) -> list[Any]:
        return value if isinstance(value, list) else []

    def _safe_string(self, value) -> str:
        return value if isinstance(value, str) else ""
