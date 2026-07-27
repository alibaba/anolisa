"""Tests for daemon background job scheduling."""

import asyncio
import contextlib
import logging
import sys
import threading
import uuid
from types import SimpleNamespace
from typing import Any

import pytest
from agent_sec_cli.correlation_context import (
    TraceContext,
    clear_process_trace_context,
    get_current_trace_context,
)
from agent_sec_cli.daemon.jobs import (
    JobManager,
    JobStatus,
    OneShotBackgroundJob,
    PeriodicBackgroundJob,
)
from agent_sec_cli.daemon.jobs.base import next_cycle_start
from agent_sec_cli.daemon.jobs.prompt_preload import (
    _PROMPT_PRELOAD_CHILD_MODULE,
    PromptModelPreloadJob,
    _download_prompt_model_sync,
    _preload_prompt_model_sync,
    _run_preload_child_process,
)
from agent_sec_cli.daemon.jobs.registry import register_default_jobs
from agent_sec_cli.daemon.runtime import PromptScanRuntimeState
from agent_sec_cli.prompt_scanner.exceptions import ModelLoadError


class RecordingPeriodicJob(PeriodicBackgroundJob):
    """Periodic job used by scheduling tests."""

    name = "recording-periodic-job"

    def __init__(self, interval_seconds: float) -> None:
        super().__init__(interval_seconds=interval_seconds)
        self.run_count = 0
        self.started = asyncio.Event()
        self.trace_contexts: list[TraceContext | None] = []

    async def run_once(self) -> None:
        """Record one scheduled run."""
        self.run_count += 1
        self.trace_contexts.append(get_current_trace_context())
        self.started.set()


class RecordingOneShotJob(OneShotBackgroundJob):
    """One-shot job used by trace-context lifecycle tests."""

    name = "recording-one-shot-job"

    def __init__(self) -> None:
        super().__init__()
        self.trace_contexts: list[tuple[str, TraceContext | None]] = []

    def on_run_started(self, started_at: str) -> None:
        self.trace_contexts.append(("started", get_current_trace_context()))

    async def run_once(self) -> None:
        """Record the active job trace context."""
        self.trace_contexts.append(("run", get_current_trace_context()))

    def on_run_completed(self, finished_at: str) -> None:
        self.trace_contexts.append(("completed", get_current_trace_context()))


class FailingOneShotJob(OneShotBackgroundJob):
    """One-shot job that fails for lifecycle logging tests."""

    name = "failing-one-shot-job"

    async def run_once(self) -> None:
        """Raise a deterministic failure."""
        raise RuntimeError("forced one-shot failure")


def _capture_job_events(monkeypatch) -> list[dict[str, Any]]:
    events: list[dict[str, Any]] = []

    def fake_log_daemon_event(**kwargs) -> None:
        if kwargs["event"].startswith("daemon_job_"):
            events.append(kwargs)

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.base.log_daemon_event",
        fake_log_daemon_event,
    )
    return events


def _assert_uuid(value: str | None) -> None:
    assert value is not None
    assert str(uuid.UUID(value)) == value


def test_next_cycle_start_uses_start_time_interval_boundaries():
    assert next_cycle_start(100.0, 103.0, 10.0) == 110.0
    assert next_cycle_start(100.0, 110.0, 10.0) == 110.0


def test_next_cycle_start_skips_missed_interval_boundaries():
    assert next_cycle_start(100.0, 112.0, 10.0) == 120.0
    assert next_cycle_start(100.0, 125.0, 10.0) == 130.0


def test_next_cycle_start_rejects_invalid_interval():
    with pytest.raises(ValueError, match="interval_seconds must be positive"):
        next_cycle_start(100.0, 101.0, 0.0)


def test_job_status_omits_unset_optional_periodic_fields():
    status = JobStatus(name="job", state="stopped")

    assert status.to_dict() == {
        "name": "job",
        "state": "stopped",
        "last_error": None,
        "last_tick_at": None,
    }


def test_periodic_background_job_runs_and_reports_interval():
    async def scenario():
        job = RecordingPeriodicJob(interval_seconds=3600.0)
        await job.start()
        try:
            await asyncio.wait_for(job.started.wait(), timeout=0.5)
            status = job.status().to_dict()
            run_count = job.run_count
        finally:
            await job.stop()
        return status, run_count

    status, run_count = asyncio.run(scenario())

    assert run_count == 1
    assert status["name"] == "recording-periodic-job"
    assert status["state"] == "running"
    assert status["interval_seconds"] == 3600.0
    assert "last_started_at" in status
    assert "next_run_at" in status


def test_one_shot_background_job_run_has_trace_context_and_resets() -> None:
    async def scenario():
        clear_process_trace_context()
        try:
            job = RecordingOneShotJob()
            await job._run_once_with_lifecycle()
            return (
                job.status().to_dict(),
                list(job.trace_contexts),
                get_current_trace_context(),
            )
        finally:
            clear_process_trace_context()

    status, trace_contexts, after_context = asyncio.run(scenario())

    labels = [label for label, _ctx in trace_contexts]
    contexts = [ctx for _label, ctx in trace_contexts]
    trace_ids = {ctx.trace_id for ctx in contexts if ctx is not None}

    assert labels == ["started", "run", "completed"]
    assert status["state"] == "completed"
    assert len(trace_ids) == 1
    trace_id = trace_ids.pop()
    _assert_uuid(trace_id)
    assert all(ctx is not None for ctx in contexts)
    assert all(ctx.session_id is None for ctx in contexts if ctx is not None)
    assert all(ctx.run_id is None for ctx in contexts if ctx is not None)
    assert all(ctx.call_id is None for ctx in contexts if ctx is not None)
    assert all(ctx.tool_call_id is None for ctx in contexts if ctx is not None)
    assert after_context is None


def test_one_shot_background_job_logs_started_and_completed(monkeypatch) -> None:
    events = _capture_job_events(monkeypatch)

    async def scenario():
        clear_process_trace_context()
        try:
            job = RecordingOneShotJob()
            await job._run_once_with_lifecycle()
            return job.status().to_dict(), get_current_trace_context()
        finally:
            clear_process_trace_context()

    status, after_context = asyncio.run(scenario())

    assert status["state"] == "completed"
    assert after_context is None
    assert [event["event"] for event in events] == [
        "daemon_job_started",
        "daemon_job_completed",
    ]
    assert events[0]["data"]["job_name"] == "recording-one-shot-job"
    assert events[0]["data"]["job_kind"] == "one_shot"
    assert events[0]["data"]["state"] == "running"
    assert events[1]["data"]["state"] == "completed"
    assert isinstance(events[1]["data"]["latency_ms"], int)
    assert events[0]["trace_context"].trace_id == events[1]["trace_context"].trace_id
    _assert_uuid(events[0]["trace_context"].trace_id)


def test_one_shot_background_job_logs_failure(monkeypatch) -> None:
    events = _capture_job_events(monkeypatch)

    async def scenario():
        clear_process_trace_context()
        try:
            job = FailingOneShotJob()
            await job._run_once_with_lifecycle()
            return job.status().to_dict(), get_current_trace_context()
        finally:
            clear_process_trace_context()

    status, after_context = asyncio.run(scenario())

    assert status["state"] == "error"
    assert status["last_error"] == "forced one-shot failure"
    assert after_context is None
    assert [event["event"] for event in events] == [
        "daemon_job_started",
        "daemon_job_failed",
    ]
    failed = events[1]
    assert failed["level"] == logging.ERROR
    assert failed["data"]["job_name"] == "failing-one-shot-job"
    assert failed["data"]["job_kind"] == "one_shot"
    assert failed["data"]["state"] == "error"
    assert failed["data"]["error_type"] == "RuntimeError"
    assert failed["data"]["error_message"] == "forced one-shot failure"
    assert isinstance(failed["data"]["latency_ms"], int)
    assert events[0]["trace_context"].trace_id == failed["trace_context"].trace_id


def test_periodic_background_job_run_gets_new_trace_context_each_tick() -> None:
    async def scenario():
        clear_process_trace_context()
        job = RecordingPeriodicJob(interval_seconds=0.01)
        try:
            await job.start()
            for _attempt in range(50):
                if len(job.trace_contexts) >= 2:
                    break
                await asyncio.sleep(0.01)
            return list(job.trace_contexts[:2]), get_current_trace_context()
        finally:
            await job.stop()
            clear_process_trace_context()

    contexts, after_context = asyncio.run(scenario())

    assert len(contexts) == 2
    assert all(ctx is not None for ctx in contexts)
    trace_ids = [ctx.trace_id for ctx in contexts if ctx is not None]
    for trace_id in trace_ids:
        _assert_uuid(trace_id)
    assert len(set(trace_ids)) == 2
    assert after_context is None


def test_periodic_background_job_logs_started_and_completed(monkeypatch) -> None:
    events = _capture_job_events(monkeypatch)

    async def scenario():
        clear_process_trace_context()
        job = RecordingPeriodicJob(interval_seconds=3600.0)
        try:
            await job.start()
            await asyncio.wait_for(job.started.wait(), timeout=0.5)
            return job.status().to_dict(), get_current_trace_context()
        finally:
            await job.stop()
            clear_process_trace_context()

    status, after_context = asyncio.run(scenario())

    assert status["state"] == "running"
    assert after_context is None
    assert [event["event"] for event in events] == [
        "daemon_job_started",
        "daemon_job_completed",
    ]
    assert events[0]["data"]["job_name"] == "recording-periodic-job"
    assert events[0]["data"]["job_kind"] == "periodic"
    assert events[0]["data"]["state"] == "running"
    assert events[0]["data"]["interval_seconds"] == 3600.0
    assert events[1]["data"]["state"] == "running"
    assert events[1]["data"]["interval_seconds"] == 3600.0
    assert isinstance(events[1]["data"]["latency_ms"], int)
    assert events[0]["trace_context"].trace_id == events[1]["trace_context"].trace_id
    _assert_uuid(events[0]["trace_context"].trace_id)


def test_register_default_jobs_respects_prompt_preload_env(monkeypatch):
    prompt_state = PromptScanRuntimeState()

    disabled_manager = JobManager()
    monkeypatch.setenv("AGENT_SEC_DAEMON_PROMPT_PRELOAD", "0")
    register_default_jobs(disabled_manager, prompt_state)

    enabled_manager = JobManager()
    monkeypatch.setenv("AGENT_SEC_DAEMON_PROMPT_PRELOAD", "1")
    register_default_jobs(enabled_manager, prompt_state)

    assert [job["name"] for job in disabled_manager.status()] == [
        "skill-ledger-activation"
    ]
    assert [job["name"] for job in enabled_manager.status()] == [
        "skill-ledger-activation",
        "prompt-model-preload",
    ]


def test_prompt_model_preload_job_cache_hit_skips_download(monkeypatch):
    prompt_state = PromptScanRuntimeState()
    child_calls: list[str] = []
    calls: list[tuple[str, str]] = []

    async def fake_child_preload(mode: str) -> None:
        child_calls.append(mode)
        raise AssertionError("cache hit should not start the download child")

    def fake_preload(state, mode: str, probe_text: str) -> None:
        calls.append((mode, probe_text))
        assert state.status == "loading"
        state.model = "fake-model"

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._run_preload_child_process",
        fake_child_preload,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._preload_prompt_model_sync",
        fake_preload,
    )

    async def scenario():
        job = PromptModelPreloadJob(prompt_state, probe_text="probe")
        await job.start()
        status = await _wait_for_job_state(job, {"completed", "error"})
        await job.stop()
        return status

    status = asyncio.run(scenario())

    assert child_calls == []
    assert calls == [("strict", "probe")]
    assert status["state"] == "completed"
    assert status["last_error"] is None
    assert prompt_state.status == "ready"
    assert prompt_state.model == "fake-model"
    assert prompt_state.loaded is True
    assert prompt_state.last_error is None
    assert prompt_state.last_started_at is not None
    assert prompt_state.last_finished_at is not None


def test_prompt_model_preload_job_propagates_trace_context_to_preload_thread(
    monkeypatch,
) -> None:
    prompt_state = PromptScanRuntimeState()
    trace_contexts: list[tuple[str, TraceContext | None]] = []
    preload_calls = 0

    async def fake_child_preload(_mode: str) -> None:
        assert prompt_state.status == "downloading"
        trace_contexts.append(("child", get_current_trace_context()))

    def fake_preload(state, _mode: str, _probe_text: str) -> None:
        nonlocal preload_calls
        preload_calls += 1
        trace_contexts.append(("preload", get_current_trace_context()))
        state.model = "fake-model"
        if preload_calls == 1:
            raise RuntimeError("cache unavailable")

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._run_preload_child_process",
        fake_child_preload,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._preload_prompt_model_sync",
        fake_preload,
    )

    async def scenario():
        clear_process_trace_context()
        try:
            job = PromptModelPreloadJob(prompt_state, probe_text="probe")
            await job.start()
            status = await _wait_for_job_state(job, {"completed", "error"})
            await job.stop()
            return status, list(trace_contexts), get_current_trace_context()
        finally:
            clear_process_trace_context()

    status, observed_contexts, after_context = asyncio.run(scenario())

    labels = [label for label, _ctx in observed_contexts]
    contexts = [ctx for _label, ctx in observed_contexts]
    trace_ids = {ctx.trace_id for ctx in contexts if ctx is not None}

    assert status["state"] == "completed"
    assert labels == ["preload", "child", "preload"]
    assert all(ctx is not None for ctx in contexts)
    assert len(trace_ids) == 1
    trace_id = trace_ids.pop()
    _assert_uuid(trace_id)
    assert all(ctx.session_id is None for ctx in contexts if ctx is not None)
    assert all(ctx.run_id is None for ctx in contexts if ctx is not None)
    assert all(ctx.call_id is None for ctx in contexts if ctx is not None)
    assert all(ctx.tool_call_id is None for ctx in contexts if ctx is not None)
    assert after_context is None


def test_prompt_model_preload_job_retries_unexpected_failure_once_then_degrades(
    monkeypatch,
):
    prompt_state = PromptScanRuntimeState()
    child_calls = 0
    preload_calls = 0

    async def fake_child_preload(_mode: str) -> None:
        nonlocal child_calls
        child_calls += 1

    def fake_preload(_state, _mode: str, _probe_text: str) -> None:
        nonlocal preload_calls
        preload_calls += 1
        raise RuntimeError("forced preload failure")

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._run_preload_child_process",
        fake_child_preload,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._preload_prompt_model_sync",
        fake_preload,
    )

    async def scenario():
        job = PromptModelPreloadJob(prompt_state)
        await job.start()
        status = await _wait_for_job_state(job, {"completed", "error"})
        await job.stop()
        return status

    status = asyncio.run(scenario())

    assert status["state"] == "error"
    assert status["last_error"] == "forced preload failure"
    assert prompt_state.status == "degraded"
    assert prompt_state.loaded is False
    assert prompt_state.last_error == "forced preload failure"
    assert prompt_state.last_finished_at is not None
    assert preload_calls == 2
    assert child_calls == 1


def test_prompt_model_preload_job_marks_prompt_degraded_on_child_failure(monkeypatch):
    prompt_state = PromptScanRuntimeState()
    preload_calls = 0

    async def fake_child_preload(_mode: str) -> None:
        assert prompt_state.status == "downloading"
        raise RuntimeError("forced child failure")

    def fake_preload(state, _mode: str, _probe_text: str) -> None:
        nonlocal preload_calls
        preload_calls += 1
        state.model = "fake-model"
        raise RuntimeError("cache unavailable")

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._run_preload_child_process",
        fake_child_preload,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._preload_prompt_model_sync",
        fake_preload,
    )

    async def scenario():
        job = PromptModelPreloadJob(prompt_state)
        await job.start()
        status = await _wait_for_job_state(job, {"completed", "error"})
        await job.stop()
        return status

    status = asyncio.run(scenario())

    assert status["state"] == "error"
    assert status["last_error"] == "forced child failure"
    assert prompt_state.status == "degraded"
    assert prompt_state.loaded is False
    assert prompt_state.last_error == "forced child failure"
    assert prompt_state.last_finished_at is not None
    assert preload_calls == 1


def test_prompt_model_preload_job_retries_only_once(monkeypatch):
    prompt_state = PromptScanRuntimeState()
    preload_calls = 0
    child_calls = 0

    async def fake_child_preload(_mode: str) -> None:
        nonlocal child_calls
        child_calls += 1

    def fake_preload(state, _mode: str, _probe_text: str) -> None:
        nonlocal preload_calls
        preload_calls += 1
        state.model = "fake-model"
        raise ModelLoadError(f"load failure {preload_calls}")

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._run_preload_child_process",
        fake_child_preload,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._preload_prompt_model_sync",
        fake_preload,
    )

    async def scenario():
        job = PromptModelPreloadJob(prompt_state)
        await job.start()
        status = await _wait_for_job_state(job, {"completed", "error"})
        await job.stop()
        return status

    status = asyncio.run(scenario())

    assert preload_calls == 2
    assert child_calls == 1
    assert status["state"] == "error"
    assert status["last_error"] == "load failure 2"
    assert prompt_state.status == "degraded"
    assert prompt_state.loaded is False
    assert prompt_state.last_error == "load failure 2"


def test_prompt_model_preload_job_cancel_during_child_preload(monkeypatch):
    prompt_state = PromptScanRuntimeState()
    child_started = asyncio.Event()
    child_cancelled = False
    preload_calls = 0

    async def fake_child_preload(_mode: str) -> None:
        nonlocal child_cancelled
        child_started.set()
        try:
            await asyncio.Event().wait()
        except asyncio.CancelledError:
            child_cancelled = True
            raise

    def fake_preload(state, _mode: str, _probe_text: str) -> None:
        nonlocal preload_calls
        preload_calls += 1
        state.model = "fake-model"
        raise RuntimeError("cache unavailable")

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._run_preload_child_process",
        fake_child_preload,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._preload_prompt_model_sync",
        fake_preload,
    )

    async def scenario():
        job = PromptModelPreloadJob(prompt_state)
        await job.start()
        await asyncio.wait_for(child_started.wait(), timeout=0.5)
        await job.stop()
        return job.status().to_dict()

    status = asyncio.run(scenario())

    assert child_cancelled is True
    assert status["state"] == "stopped"
    assert prompt_state.status == "stopped"
    assert prompt_state.loaded is False
    assert prompt_state.last_error is None
    assert prompt_state.last_finished_at is not None
    assert preload_calls == 1


def test_prompt_model_preload_job_cancel_during_main_preload(monkeypatch):
    prompt_state = PromptScanRuntimeState()
    preload_started = threading.Event()
    preload_finished = threading.Event()
    release_preload = threading.Event()

    async def fake_child_preload(_mode: str) -> None:
        raise AssertionError("cache hit should not start the download child")

    def fake_preload(_state, _mode: str, _probe_text: str) -> None:
        preload_started.set()
        try:
            release_preload.wait(timeout=1.0)
        finally:
            preload_finished.set()

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._run_preload_child_process",
        fake_child_preload,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload._preload_prompt_model_sync",
        fake_preload,
    )

    async def scenario():
        job = PromptModelPreloadJob(prompt_state)
        await job.start()
        for _attempt in range(50):
            if preload_started.is_set():
                break
            await asyncio.sleep(0.01)
        assert preload_started.is_set()

        await job.stop()
        release_preload.set()
        for _attempt in range(50):
            if preload_finished.is_set():
                break
            await asyncio.sleep(0.01)
        assert preload_finished.is_set()
        return job.status().to_dict(), prompt_state.to_dict()

    status, prompt_snapshot = asyncio.run(scenario())

    assert status["state"] == "stopped"
    assert prompt_snapshot["status"] == "stopped"
    assert prompt_snapshot["loaded"] is False
    assert prompt_snapshot["last_error"] is None
    assert prompt_snapshot["last_finished_at"] is not None


def test_prompt_preload_child_process_is_terminated_on_cancel(monkeypatch):
    process_started = asyncio.Event()
    subprocess_args = []

    class FakeProcess:
        def __init__(self) -> None:
            self.returncode = None
            self.terminated = False
            self.killed = False

        async def communicate(self):
            process_started.set()
            await asyncio.Event().wait()
            return b"", b""

        def terminate(self) -> None:
            self.terminated = True
            self.returncode = -15

        def kill(self) -> None:
            self.killed = True
            self.returncode = -9

        async def wait(self) -> int:
            return self.returncode

    fake_process = FakeProcess()

    async def fake_create_subprocess_exec(*args, **_kwargs):
        subprocess_args.append(args)
        return fake_process

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload.asyncio.create_subprocess_exec",
        fake_create_subprocess_exec,
    )

    async def scenario():
        task = asyncio.create_task(_run_preload_child_process("strict"))
        await asyncio.wait_for(process_started.wait(), timeout=0.5)
        task.cancel()
        with contextlib.suppress(asyncio.CancelledError):
            await task

    asyncio.run(scenario())

    assert fake_process.terminated is True
    assert fake_process.killed is False
    assert subprocess_args == [
        (sys.executable, "-m", _PROMPT_PRELOAD_CHILD_MODULE, "strict")
    ]


def test_prompt_preload_child_process_is_terminated_on_timeout(monkeypatch):
    process_started = asyncio.Event()
    subprocess_args = []

    class FakeProcess:
        def __init__(self) -> None:
            self.returncode = None
            self.terminated = False
            self.killed = False

        async def communicate(self):
            process_started.set()
            await asyncio.Event().wait()
            return b"", b""

        def terminate(self) -> None:
            self.terminated = True
            self.returncode = -15

        def kill(self) -> None:
            self.killed = True
            self.returncode = -9

        async def wait(self) -> int:
            return self.returncode

    fake_process = FakeProcess()

    async def fake_create_subprocess_exec(*args, **_kwargs):
        subprocess_args.append(args)
        return fake_process

    monkeypatch.setenv(
        "AGENT_SEC_DAEMON_PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_SECONDS",
        "0.01",
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.prompt_preload.asyncio.create_subprocess_exec",
        fake_create_subprocess_exec,
    )

    async def scenario():
        with pytest.raises(RuntimeError, match="timed out after 0.01s"):
            await _run_preload_child_process("strict")

    asyncio.run(scenario())

    assert process_started.is_set()
    assert fake_process.terminated is True
    assert fake_process.killed is False
    assert subprocess_args == [
        (sys.executable, "-m", _PROMPT_PRELOAD_CHILD_MODULE, "strict")
    ]


def test_prompt_model_download_sync_suppresses_warmup_output(monkeypatch, capsys):
    calls = []

    class FakePromptScanner:
        def __init__(self, mode):
            calls.append(("init", mode.value))

        def warmup(self):
            print("download progress on stdout")
            print("download progress on stderr", file=sys.stderr)
            calls.append(("warmup",))

        def scan(self, text, source=None):
            raise AssertionError("download-only child preload should not scan")

    monkeypatch.setattr(
        "agent_sec_cli.prompt_scanner.scanner.PromptScanner",
        FakePromptScanner,
    )

    _download_prompt_model_sync("strict")
    captured = capsys.readouterr()

    assert calls == [
        ("init", "strict"),
        ("warmup",),
    ]
    assert captured.out == ""
    assert captured.err == ""


def test_prompt_model_preload_sync_does_not_redirect_daemon_stdio(monkeypatch, capsys):
    prompt_state = PromptScanRuntimeState()
    calls = []
    original_stdout = sys.stdout
    original_stderr = sys.stderr

    class FakePromptScanner:
        def __init__(self, mode):
            calls.append(("init", mode.value))

        def warmup(self):
            raise AssertionError("daemon preload should not run download warmup")

        def scan(self, text, source=None):
            assert sys.stdout is original_stdout
            assert sys.stderr is original_stderr
            print("daemon stdout remains visible")
            print("daemon stderr remains visible", file=sys.stderr)
            calls.append(("scan", text, source))
            return SimpleNamespace(
                layer_results=[SimpleNamespace(layer_name="ml_classifier")]
            )

    monkeypatch.setattr(
        "agent_sec_cli.prompt_scanner.scanner.PromptScanner",
        FakePromptScanner,
    )

    _preload_prompt_model_sync(prompt_state, "strict", "probe")
    captured = capsys.readouterr()

    assert calls == [
        ("init", "strict"),
        ("scan", "probe", "daemon-startup"),
    ]
    assert captured.out == "daemon stdout remains visible\n"
    assert captured.err == "daemon stderr remains visible\n"
    assert prompt_state.model == "LLM-Research/Llama-Prompt-Guard-2-86M"
    assert prompt_state.status == "pending"


def test_prompt_model_preload_sync_requires_ml_probe_result(monkeypatch):
    prompt_state = PromptScanRuntimeState()

    class FakePromptScanner:
        def __init__(self, mode):
            assert mode.value == "strict"

        def scan(self, _text, source=None):
            assert source == "daemon-startup"
            return SimpleNamespace(
                layer_results=[SimpleNamespace(layer_name="rule_engine")]
            )

    monkeypatch.setattr(
        "agent_sec_cli.prompt_scanner.scanner.PromptScanner",
        FakePromptScanner,
    )

    with pytest.raises(
        RuntimeError,
        match="prompt model probe did not execute the ML classifier",
    ):
        _preload_prompt_model_sync(prompt_state, "strict", "probe")


def test_prompt_model_preload_sync_propagates_model_load_error(monkeypatch):
    prompt_state = PromptScanRuntimeState()

    class FakePromptScanner:
        def __init__(self, mode):
            assert mode.value == "strict"

        def scan(self, _text, source=None):
            assert source == "daemon-startup"
            raise ModelLoadError("cache unavailable")

    monkeypatch.setattr(
        "agent_sec_cli.prompt_scanner.scanner.PromptScanner",
        FakePromptScanner,
    )

    with pytest.raises(ModelLoadError, match="cache unavailable"):
        _preload_prompt_model_sync(prompt_state, "strict", "probe")


async def _wait_for_job_state(
    job: PromptModelPreloadJob,
    target_states: set[str],
) -> dict:
    for _attempt in range(50):
        status = job.status().to_dict()
        if status["state"] in target_states:
            return status
        await asyncio.sleep(0.01)
    return job.status().to_dict()
