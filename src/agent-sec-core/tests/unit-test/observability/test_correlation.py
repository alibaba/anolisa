"""Unit tests for observability-to-security-event correlation."""

from dataclasses import dataclass

import pytest
from agent_sec_cli.observability.correlation import (
    ZERO_RUN_ID,
    ObservabilityRecordFields,
    SecurityCorrelationService,
)
from agent_sec_cli.security_events.schema import SecurityEvent


@dataclass(frozen=True)
class _Candidate:
    event: SecurityEvent
    timestamp_epoch: float


class _FakeReader:
    def __init__(self, candidates: list[_Candidate] | None = None) -> None:
        self.candidates = candidates or []
        self.calls: list[dict[str, object]] = []

    def query_correlation_candidates(self, **kwargs: object) -> list[_Candidate]:
        self.calls.append(kwargs)
        return self.candidates


def _record(
    *,
    hook: str = "before_tool_call",
    session_id: str | None = "session-1",
    run_id: str | None = "run-1",
    tool_call_id: str | None = "tool-1",
    observed_at_epoch: float = 100.0,
) -> ObservabilityRecordFields:
    return ObservabilityRecordFields(
        hook=hook,
        session_id=session_id,
        run_id=run_id,
        tool_call_id=tool_call_id,
        observed_at_epoch=observed_at_epoch,
    )


def _event(
    *,
    event_id: str,
    category: str,
    timestamp: str = "2026-05-20T00:00:00+00:00",
    session_id: str | None = "session-1",
    run_id: str | None = "run-1",
    tool_call_id: str | None = "tool-1",
) -> SecurityEvent:
    return SecurityEvent(
        event_id=event_id,
        event_type=category,
        category=category,
        result="succeeded",
        timestamp=timestamp,
        trace_id="trace-ignored",
        pid=1,
        uid=1,
        session_id=session_id,
        run_id=run_id,
        tool_call_id=tool_call_id,
        details={"event": event_id},
    )


@pytest.mark.parametrize(
    "hook",
    [
        "before_llm_call",
        "after_llm_call",
        "after_tool_call",
        "after_agent_run",
    ],
)
def test_unsupported_hook_returns_empty_without_reader_call(hook: str) -> None:
    reader = _FakeReader()
    service = SecurityCorrelationService(reader)

    result = service.find_correlated(_record(hook=hook))

    assert result == []
    assert reader.calls == []


def test_missing_session_returns_empty_without_reader_call() -> None:
    reader = _FakeReader()
    service = SecurityCorrelationService(reader)

    result = service.find_correlated(_record(session_id=None))

    assert result == []
    assert reader.calls == []


def test_exact_mode_uses_tool_call_id_without_time_window_and_orders_categories() -> (
    None
):
    reader = _FakeReader(
        [
            _Candidate(
                _event(event_id="skill-later", category="skill_ledger"),
                timestamp_epoch=150.0,
            ),
            _Candidate(
                _event(event_id="code-far-away", category="code_scan"),
                timestamp_epoch=1000.0,
            ),
            _Candidate(
                _event(event_id="skill-nearer", category="skill_ledger"),
                timestamp_epoch=110.0,
            ),
            _Candidate(
                _event(event_id="prompt-disallowed", category="prompt_scan"),
                timestamp_epoch=100.0,
            ),
        ]
    )
    service = SecurityCorrelationService(reader)

    result = service.find_correlated(_record(observed_at_epoch=100.0))

    assert reader.calls == [
        {
            "session_id": "session-1",
            "categories": ("code_scan", "skill_ledger"),
            "run_id": "run-1",
            "tool_call_id": "tool-1",
            "since_epoch": None,
            "until_epoch": None,
        }
    ]
    assert [item.event.event_id for item in result] == [
        "code-far-away",
        "skill-nearer",
    ]
    assert [item.match_reason for item in result] == ["tool_call_id", "tool_call_id"]
    assert [item.time_delta_seconds for item in result] == [900.0, 10.0]


def test_exact_mode_does_not_fallback_when_no_exact_candidates() -> None:
    reader = _FakeReader()
    service = SecurityCorrelationService(reader)

    result = service.find_correlated(_record(observed_at_epoch=100.0))

    assert result == []
    assert reader.calls == [
        {
            "session_id": "session-1",
            "categories": ("code_scan", "skill_ledger"),
            "run_id": "run-1",
            "tool_call_id": "tool-1",
            "since_epoch": None,
            "until_epoch": None,
        }
    ]


def test_exact_mode_rejects_candidates_with_missing_security_correlation_fields() -> (
    None
):
    reader = _FakeReader(
        [
            _Candidate(
                _event(event_id="missing-session", category="code_scan", session_id=""),
                timestamp_epoch=100.0,
            ),
            _Candidate(
                _event(event_id="missing-run", category="code_scan", run_id=None),
                timestamp_epoch=100.0,
            ),
            _Candidate(
                _event(
                    event_id="missing-tool", category="skill_ledger", tool_call_id=""
                ),
                timestamp_epoch=100.0,
            ),
        ]
    )
    service = SecurityCorrelationService(reader)

    result = service.find_correlated(_record(observed_at_epoch=100.0))

    assert result == []


@pytest.mark.parametrize("category", ["sandbox", "hardening", "asset_verify"])
@pytest.mark.parametrize("mode", ["exact", "fallback"])
def test_categories_outside_security_mapping_are_filtered(
    mode: str, category: str
) -> None:
    reader = _FakeReader(
        [
            _Candidate(
                _event(event_id=f"{mode}-{category}", category=category),
                timestamp_epoch=100.0,
            )
        ]
    )
    service = SecurityCorrelationService(reader)
    record = (
        _record(observed_at_epoch=100.0)
        if mode == "exact"
        else _record(tool_call_id=None, observed_at_epoch=100.0)
    )

    result = service.find_correlated(record)

    assert result == []


def test_before_agent_run_uses_run_id_match_without_time_window() -> None:
    reader = _FakeReader(
        [
            _Candidate(
                _event(event_id="prompt-slow", category="prompt_scan"),
                timestamp_epoch=106.0,
            ),
            _Candidate(
                _event(event_id="pii-near", category="pii_scan"),
                timestamp_epoch=101.0,
            ),
            _Candidate(
                _event(
                    event_id="prompt-wrong-run",
                    category="prompt_scan",
                    run_id="run-2",
                ),
                timestamp_epoch=100.5,
            ),
        ]
    )
    service = SecurityCorrelationService(reader)

    result = service.find_correlated(
        _record(
            hook="before_agent_run",
            tool_call_id=None,
            observed_at_epoch=100.0,
        )
    )

    assert reader.calls == [
        {
            "session_id": "session-1",
            "categories": ("prompt_scan", "pii_scan"),
            "run_id": "run-1",
            "tool_call_id": None,
            "since_epoch": None,
            "until_epoch": None,
        }
    ]
    assert [item.event.event_id for item in result] == ["prompt-slow", "pii-near"]
    assert [item.match_reason for item in result] == ["run_id", "run_id"]
    assert [item.time_delta_seconds for item in result] == [6.0, 1.0]


def test_fallback_mode_uses_session_only_for_zero_run_and_keeps_closest_per_category() -> (
    None
):
    reader = _FakeReader(
        [
            _Candidate(
                _event(event_id="prompt-near", category="prompt_scan", run_id="run-A"),
                timestamp_epoch=101.0,
            ),
            _Candidate(
                _event(event_id="prompt-far", category="prompt_scan", run_id="run-B"),
                timestamp_epoch=101.5,
            ),
            _Candidate(
                _event(event_id="pii-boundary", category="pii_scan", run_id="run-C"),
                timestamp_epoch=102.0,
            ),
            _Candidate(
                _event(
                    event_id="code-disallowed", category="code_scan", run_id="run-D"
                ),
                timestamp_epoch=100.0,
            ),
        ]
    )
    service = SecurityCorrelationService(reader)

    result = service.find_correlated(
        _record(
            hook="before_agent_run",
            run_id=ZERO_RUN_ID,
            tool_call_id=None,
            observed_at_epoch=100.0,
        )
    )

    assert reader.calls == [
        {
            "session_id": "session-1",
            "categories": ("prompt_scan", "pii_scan"),
            "run_id": None,
            "tool_call_id": None,
            "since_epoch": 98.0,
            "until_epoch": 102.0,
        }
    ]
    assert [item.event.event_id for item in result] == ["prompt-near", "pii-boundary"]
    assert [item.event.run_id for item in result] == ["run-A", "run-C"]
    assert [item.match_reason for item in result] == ["rule+time", "rule+time"]
    assert [item.time_delta_seconds for item in result] == [1.0, 2.0]


@pytest.mark.parametrize("run_id", [None, ""])
def test_fallback_mode_uses_session_only_when_run_id_is_missing(
    run_id: str | None,
) -> None:
    reader = _FakeReader(
        [
            _Candidate(
                _event(
                    event_id="cross-run-match",
                    category="code_scan",
                    run_id="security-run",
                ),
                timestamp_epoch=100.5,
            )
        ]
    )
    service = SecurityCorrelationService(reader)

    result = service.find_correlated(
        _record(run_id=run_id, tool_call_id=None, observed_at_epoch=100.0)
    )

    assert reader.calls == [
        {
            "session_id": "session-1",
            "categories": ("code_scan", "skill_ledger"),
            "run_id": None,
            "tool_call_id": None,
            "since_epoch": 98.0,
            "until_epoch": 102.0,
        }
    ]
    assert [item.event.event_id for item in result] == ["cross-run-match"]
    assert result[0].event.run_id == "security-run"


def test_fallback_mode_filters_candidates_outside_time_window_from_defensive_reader() -> (
    None
):
    reader = _FakeReader(
        [
            _Candidate(
                _event(event_id="inside", category="code_scan"),
                timestamp_epoch=101.9,
            ),
            _Candidate(
                _event(event_id="outside", category="skill_ledger"),
                timestamp_epoch=102.1,
            ),
        ]
    )
    service = SecurityCorrelationService(reader)

    result = service.find_correlated(
        _record(tool_call_id=None, observed_at_epoch=100.0)
    )

    assert reader.calls[0]["run_id"] == "run-1"
    assert [item.event.event_id for item in result] == ["inside"]
