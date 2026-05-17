"""Unit tests for the observability review TUI."""

import asyncio
import json

from agent_sec_cli.observability.models import ObservabilityEventRecord
from agent_sec_cli.observability.repositories import RunSummary, SessionSummary
from agent_sec_cli.observability.review import (
    EventDetailScreen,
    EventListScreen,
    ObservabilityReviewApp,
    SessionListScreen,
    TurnListScreen,
    _safe_pretty_json,
    _summarize_metrics,
)
from textual.app import App
from textual.widgets import DataTable, Static


class _FakeReader:
    def __init__(
        self,
        *,
        sessions: list[SessionSummary] | None = None,
        runs_by_session: dict[str, list[RunSummary]] | None = None,
        events_by_run: (
            dict[tuple[str, str], list[ObservabilityEventRecord]] | None
        ) = None,
    ) -> None:
        self.sessions = sessions or []
        self.runs_by_session = runs_by_session or {}
        self.events_by_run = events_by_run or {}
        self.calls: list[tuple[str, tuple[str, ...]]] = []

    def list_sessions(self) -> list[SessionSummary]:
        self.calls.append(("list_sessions", ()))
        return self.sessions

    def list_runs(self, session_id: str) -> list[RunSummary]:
        self.calls.append(("list_runs", (session_id,)))
        return self.runs_by_session.get(session_id, [])

    def list_events(
        self, session_id: str, run_id: str
    ) -> list[ObservabilityEventRecord]:
        self.calls.append(("list_events", (session_id, run_id)))
        return self.events_by_run.get((session_id, run_id), [])


def _record(
    *,
    record_id: int = 1,
    hook: str = "before_agent_run",
    observed_at: str = "2026-05-16T12:00:00Z",
    observed_at_epoch: float = 1778932800.0,
    metrics: dict[str, object] | None = None,
    metadata: dict[str, object] | None = None,
    call_id: str | None = None,
    tool_call_id: str | None = None,
) -> ObservabilityEventRecord:
    metadata_payload = metadata or {"sessionId": "session-A", "runId": "run-A"}
    return ObservabilityEventRecord(
        id=record_id,
        hook=hook,
        observed_at=observed_at,
        observed_at_epoch=observed_at_epoch,
        session_id=str(metadata_payload["sessionId"]),
        run_id=str(metadata_payload["runId"]),
        metrics_json=json.dumps(metrics or {"prompt": "hello"}),
        metadata_json=json.dumps(metadata_payload),
        call_id=call_id,
        tool_call_id=tool_call_id,
    )


def _render_detail_text(record: ObservabilityEventRecord) -> str:
    async def render() -> str:
        app = App()
        async with app.run_test() as pilot:
            await app.push_screen(EventDetailScreen(record=record))
            await pilot.pause()
            return "\n".join(
                str(widget.render()) for widget in app.screen.query(Static)
            )

    return asyncio.run(render())


def test_event_detail_renders_markup_like_record_data_literally() -> None:
    text = _render_detail_text(
        _record(
            metrics={
                "prompt": "explain [red] in CSS",
                "result": "removed lines: [/]",
            },
            metadata={
                "sessionId": "session-A",
                "runId": "run-A",
                "note": "[link=https://example.invalid]click[/link]",
            },
        )
    )

    assert "explain [red] in CSS" in text
    assert "removed lines: [/]" in text
    assert "[link=https://example.invalid]click[/link]" in text


def test_event_detail_shows_true_utc_timestamp_for_non_utc_observed_at() -> None:
    text = _render_detail_text(
        _record(
            observed_at="2026-05-16T20:00:00+08:00",
            observed_at_epoch=1778932800.0,
        )
    )

    assert "2026-05-16T12:00:00+00:00" in text
    assert "2026-05-16T20:00:00+08:00 UTC" not in text


def test_event_detail_renders_optional_call_identifiers() -> None:
    text = _render_detail_text(_record(call_id="call-1", tool_call_id="tool-call-1"))

    assert "call-1" in text
    assert "tool-call-1" in text


def test_review_app_drills_from_session_to_event_detail() -> None:
    async def run() -> tuple[list[tuple[str, tuple[str, ...]]], str]:
        record = _record(
            record_id=42,
            hook="before_tool_call",
            metrics={"tool_name": "grep"},
            metadata={
                "sessionId": "session-alpha-long-enough-to-truncate-in-list",
                "runId": "run-alpha-long-enough-to-truncate-in-list",
                "toolCallId": "tool-call-1",
            },
            tool_call_id="tool-call-1",
        )
        reader = _FakeReader(
            sessions=[
                SessionSummary(
                    session_id="session-alpha-long-enough-to-truncate-in-list",
                    first_seen_epoch=1778932700.0,
                    last_seen_epoch=1778932800.0,
                    turn_count=1,
                    event_count=1,
                )
            ],
            runs_by_session={
                "session-alpha-long-enough-to-truncate-in-list": [
                    RunSummary(
                        run_id="run-alpha-long-enough-to-truncate-in-list",
                        started_at_epoch=1778932750.0,
                        ended_at_epoch=1778932800.0,
                        user_input_preview="summarize the repository",
                        event_count=1,
                    )
                ]
            },
            events_by_run={
                (
                    "session-alpha-long-enough-to-truncate-in-list",
                    "run-alpha-long-enough-to-truncate-in-list",
                ): [record]
            },
        )
        app = ObservabilityReviewApp(reader=reader)  # type: ignore[arg-type]

        async with app.run_test() as pilot:
            await pilot.pause()
            assert isinstance(app.screen, SessionListScreen)
            session_table = app.screen.query_one(DataTable)
            assert session_table.row_count == 1

            await pilot.press("enter")
            await pilot.pause()
            assert isinstance(app.screen, TurnListScreen)
            turn_table = app.screen.query_one(DataTable)
            assert turn_table.row_count == 1

            await pilot.press("enter")
            await pilot.pause()
            assert isinstance(app.screen, EventListScreen)
            event_table = app.screen.query_one(DataTable)
            assert event_table.row_count == 1

            await pilot.press("enter")
            await pilot.pause()
            assert isinstance(app.screen, EventDetailScreen)
            detail_text = "\n".join(
                str(widget.render()) for widget in app.screen.query(Static)
            )

        return reader.calls, detail_text

    calls, detail_text = asyncio.run(run())

    assert calls == [
        ("list_sessions", ()),
        ("list_runs", ("session-alpha-long-enough-to-truncate-in-list",)),
        (
            "list_events",
            (
                "session-alpha-long-enough-to-truncate-in-list",
                "run-alpha-long-enough-to-truncate-in-list",
            ),
        ),
    ]
    assert "before_tool_call" in detail_text
    assert "tool-call-1" in detail_text


def test_non_root_back_pops_to_previous_screen() -> None:
    async def run() -> bool:
        app = ObservabilityReviewApp(reader=_FakeReader())  # type: ignore[arg-type]
        async with app.run_test() as pilot:
            await app.push_screen(TurnListScreen(session_id="session-A"))
            await pilot.pause()
            await pilot.press("escape")
            await pilot.pause()
            return isinstance(app.screen, SessionListScreen)

    assert asyncio.run(run()) is True


def test_review_app_empty_session_list_shows_placeholder() -> None:
    async def run() -> tuple[str, bool]:
        app = ObservabilityReviewApp(reader=_FakeReader())  # type: ignore[arg-type]
        async with app.run_test() as pilot:
            await pilot.pause()
            empty = app.screen.query_one("#empty", Static)
            table = app.screen.query_one(DataTable)
            return str(empty.render()), bool(table.display)

    message, table_display = asyncio.run(run())

    assert message == "No observability records found."
    assert table_display is False


def test_turn_list_empty_state_shows_placeholder() -> None:
    async def run() -> tuple[str, bool]:
        app = ObservabilityReviewApp(
            reader=_FakeReader(runs_by_session={"session-A": []})  # type: ignore[arg-type]
        )
        async with app.run_test() as pilot:
            await app.push_screen(TurnListScreen(session_id="session-A"))
            await pilot.pause()
            empty = app.screen.query_one("#empty", Static)
            table = app.screen.query_one(DataTable)
            return str(empty.render()), bool(table.display)

    message, table_display = asyncio.run(run())

    assert message == "No runs recorded for this session."
    assert table_display is False


def test_event_list_empty_state_shows_placeholder() -> None:
    async def run() -> tuple[str, bool]:
        app = ObservabilityReviewApp(reader=_FakeReader())  # type: ignore[arg-type]
        async with app.run_test() as pilot:
            await app.push_screen(
                EventListScreen(session_id="session-A", run_id="run-A")
            )
            await pilot.pause()
            empty = app.screen.query_one("#empty", Static)
            table = app.screen.query_one(DataTable)
            return str(empty.render()), bool(table.display)

    message, table_display = asyncio.run(run())

    assert message == "No events for this run."
    assert table_display is False


def test_event_list_ignores_stale_row_key() -> None:
    async def run() -> bool:
        app = ObservabilityReviewApp(reader=_FakeReader())  # type: ignore[arg-type]
        async with app.run_test() as pilot:
            screen = EventListScreen(session_id="session-A", run_id="run-A")
            await app.push_screen(screen)
            await pilot.pause()
            screen._rows_by_key = {}
            screen._drill("missing-row")
            await pilot.pause()
            return isinstance(app.screen, EventListScreen)

    assert asyncio.run(run()) is True


def test_summarize_metrics_renders_hook_specific_timeline_text() -> None:
    assert (
        _summarize_metrics(
            "before_agent_run", json.dumps({"user_input": "review this diff"})
        )
        == "review this diff"
    )
    assert (
        _summarize_metrics("before_llm_call", json.dumps({"model_provider": "openai"}))
        == "model=openai"
    )
    assert (
        _summarize_metrics(
            "after_llm_call", json.dumps({"latency_ms": 25, "outcome": "ok"})
        )
        == "latency=25ms ok"
    )
    assert (
        _summarize_metrics("before_tool_call", json.dumps({"tool_name": "rg"}))
        == "tool=rg"
    )
    assert (
        _summarize_metrics(
            "after_tool_call", json.dumps({"duration_ms": 7, "error": "boom"})
        )
        == "status=err duration=7ms"
    )
    assert (
        _summarize_metrics(
            "after_agent_run", json.dumps({"success": True, "duration_ms": 91})
        )
        == "success=True duration=91ms"
    )


def test_summarize_metrics_handles_unreadable_rows() -> None:
    assert _summarize_metrics("before_agent_run", "{") == "(unparseable metrics)"
    assert _summarize_metrics("before_agent_run", json.dumps(["not", "object"])) == (
        "(non-object metrics)"
    )
    assert _summarize_metrics("future_hook", json.dumps({"value": "x"})) == ""


def test_safe_pretty_json_falls_back_to_raw_snippet_for_malformed_json() -> None:
    raw = "{" + ("x" * 600)

    rendered = _safe_pretty_json(raw)

    assert rendered.startswith("Failed to parse JSON:\n{")
    assert len(rendered) < len(raw) + 30
