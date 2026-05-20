"""Correlate observability records to security events for review UI."""

from dataclasses import dataclass
from typing import Literal, Protocol

from agent_sec_cli.security_events.schema import SecurityEvent

ZERO_RUN_ID = "00000000-0000-0000-0000-000000000000"
FALLBACK_TIME_WINDOW_SECONDS = 2.0

SUPPORTED_SECURITY_EVENT_CATEGORIES: dict[str, tuple[str, ...]] = {
    "before_tool_call": ("code_scan", "skill_ledger"),
    "before_agent_run": ("prompt_scan", "pii_scan"),
}

MatchReason = Literal["tool_call_id", "run_id", "rule+time"]


@dataclass(frozen=True)
class ObservabilityRecordFields:
    """Plain fields required to correlate one observability record."""

    hook: str
    session_id: str | None
    run_id: str | None
    tool_call_id: str | None
    observed_at_epoch: float


@dataclass(frozen=True)
class CorrelatedSecurityEvent:
    """Security event plus correlation metadata computed by the service."""

    event: SecurityEvent
    match_reason: MatchReason
    time_delta_seconds: float
    security_timestamp_epoch: float


class _SecurityEventCandidate(Protocol):
    event: SecurityEvent
    timestamp_epoch: float


class _CorrelationReader(Protocol):
    def query_correlation_candidates(
        self,
        *,
        session_id: str,
        categories: tuple[str, ...],
        run_id: str | None,
        tool_call_id: str | None,
        since_epoch: float | None,
        until_epoch: float | None,
    ) -> list[_SecurityEventCandidate]:
        pass


class SecurityCorrelationService:
    """Find security events correlated to one observability record."""

    def __init__(self, reader: _CorrelationReader) -> None:
        self._reader = reader

    def find_correlated(
        self, record: ObservabilityRecordFields
    ) -> list[CorrelatedSecurityEvent]:
        """Return sorted, category-deduplicated security-event correlations."""
        categories = SUPPORTED_SECURITY_EVENT_CATEGORIES.get(record.hook)
        if categories is None or _missing(record.session_id):
            return []

        if _has_tool_call_correlation(record):
            candidates = self._reader.query_correlation_candidates(
                session_id=str(record.session_id),
                categories=categories,
                run_id=record.run_id,
                tool_call_id=record.tool_call_id,
                since_epoch=None,
                until_epoch=None,
            )
            return self._select_by_category(
                record,
                candidates,
                categories,
                "tool_call_id",
            )

        if _has_run_correlation(record):
            candidates = self._reader.query_correlation_candidates(
                session_id=str(record.session_id),
                categories=categories,
                run_id=record.run_id,
                tool_call_id=None,
                since_epoch=None,
                until_epoch=None,
            )
            return self._select_by_category(
                record,
                candidates,
                categories,
                "run_id",
            )

        run_id = None if _missing_run_id(record.run_id) else record.run_id
        candidates = self._reader.query_correlation_candidates(
            session_id=str(record.session_id),
            categories=categories,
            run_id=run_id,
            tool_call_id=None,
            since_epoch=record.observed_at_epoch - FALLBACK_TIME_WINDOW_SECONDS,
            until_epoch=record.observed_at_epoch + FALLBACK_TIME_WINDOW_SECONDS,
        )
        return self._select_by_category(record, candidates, categories, "rule+time")

    def _select_by_category(
        self,
        record: ObservabilityRecordFields,
        candidates: list[_SecurityEventCandidate],
        categories: tuple[str, ...],
        match_reason: MatchReason,
    ) -> list[CorrelatedSecurityEvent]:
        selected: dict[str, CorrelatedSecurityEvent] = {}
        for candidate in candidates:
            if not _candidate_matches(record, candidate, categories, match_reason):
                continue
            correlated = CorrelatedSecurityEvent(
                event=candidate.event,
                match_reason=match_reason,
                time_delta_seconds=candidate.timestamp_epoch - record.observed_at_epoch,
                security_timestamp_epoch=candidate.timestamp_epoch,
            )
            current = selected.get(candidate.event.category)
            if current is None or _rank(correlated) < _rank(current):
                selected[candidate.event.category] = correlated

        return [selected[category] for category in categories if category in selected]


def _candidate_matches(
    record: ObservabilityRecordFields,
    candidate: _SecurityEventCandidate,
    categories: tuple[str, ...],
    match_reason: MatchReason,
) -> bool:
    event = candidate.event
    if event.category not in categories:
        return False
    if _missing(event.session_id) or event.session_id != record.session_id:
        return False

    if match_reason == "tool_call_id":
        return (
            not _missing_run_id(event.run_id)
            and event.run_id == record.run_id
            and not _missing(event.tool_call_id)
            and event.tool_call_id == record.tool_call_id
        )

    if match_reason == "run_id":
        return not _missing_run_id(event.run_id) and event.run_id == record.run_id

    if not _missing_run_id(record.run_id) and event.run_id != record.run_id:
        return False
    return (
        abs(candidate.timestamp_epoch - record.observed_at_epoch)
        <= FALLBACK_TIME_WINDOW_SECONDS
    )


def _has_tool_call_correlation(record: ObservabilityRecordFields) -> bool:
    return (
        not _missing(record.session_id)
        and not _missing_run_id(record.run_id)
        and not _missing(record.tool_call_id)
    )


def _has_run_correlation(record: ObservabilityRecordFields) -> bool:
    return record.hook == "before_agent_run" and not _missing_run_id(record.run_id)


def _missing(value: str | None) -> bool:
    return value is None or not value.strip()


def _missing_run_id(value: str | None) -> bool:
    return _missing(value) or value == ZERO_RUN_ID


def _rank(correlation: CorrelatedSecurityEvent) -> tuple[float, float, str]:
    return (
        abs(correlation.time_delta_seconds),
        correlation.security_timestamp_epoch,
        correlation.event.event_id,
    )


__all__ = [
    "CorrelatedSecurityEvent",
    "FALLBACK_TIME_WINDOW_SECONDS",
    "ObservabilityRecordFields",
    "SUPPORTED_SECURITY_EVENT_CATEGORIES",
    "SecurityCorrelationService",
    "ZERO_RUN_ID",
]
