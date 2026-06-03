"""Tests for daemon background job scheduling."""

import asyncio
import sys

import pytest
from agent_sec_cli.daemon.jobs import (
    JobManager,
    JobStatus,
    PeriodicBackgroundJob,
)
from agent_sec_cli.daemon.jobs.base import next_cycle_start
from agent_sec_cli.daemon.jobs.prompt_preload import (
    PromptModelPreloadJob,
    _preload_prompt_model_sync,
)
from agent_sec_cli.daemon.jobs.registry import register_default_jobs
from agent_sec_cli.daemon.runtime import PromptRuntimeState


class RecordingPeriodicJob(PeriodicBackgroundJob):
    """Periodic job used by scheduling tests."""

    name = "recording-periodic-job"

    def __init__(self, interval_seconds: float) -> None:
        super().__init__(interval_seconds=interval_seconds)
        self.run_count = 0
        self.started = asyncio.Event()

    async def run_once(self) -> None:
        """Record one scheduled run."""
        self.run_count += 1
        self.started.set()


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


def test_register_default_jobs_respects_prompt_preload_env(monkeypatch):
    prompt_state = PromptRuntimeState()

    disabled_manager = JobManager()
    monkeypatch.setenv("AGENT_SEC_DAEMON_PROMPT_PRELOAD", "0")
    register_default_jobs(disabled_manager, prompt_state)

    enabled_manager = JobManager()
    monkeypatch.setenv("AGENT_SEC_DAEMON_PROMPT_PRELOAD", "1")
    register_default_jobs(enabled_manager, prompt_state)

    assert disabled_manager.status() == []
    assert [job["name"] for job in enabled_manager.status()] == ["prompt-model-preload"]


def test_prompt_model_preload_job_updates_runtime_state(monkeypatch):
    prompt_state = PromptRuntimeState()
    calls: list[tuple[str, str]] = []

    def fake_preload(state, mode: str, probe_text: str) -> None:
        calls.append((mode, probe_text))
        assert state.status == "downloading"
        state.model = "fake-model"
        state.status = "loading"

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

    assert calls == [("strict", "probe")]
    assert status["state"] == "completed"
    assert status["last_error"] is None
    assert prompt_state.status == "ready"
    assert prompt_state.model == "fake-model"
    assert prompt_state.loaded is True
    assert prompt_state.last_error is None
    assert prompt_state.last_started_at is not None
    assert prompt_state.last_finished_at is not None


def test_prompt_model_preload_job_marks_prompt_degraded_on_failure(monkeypatch):
    prompt_state = PromptRuntimeState()

    def fake_preload(_state, _mode: str, _probe_text: str) -> None:
        raise RuntimeError("forced preload failure")

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


def test_prompt_model_preload_sync_suppresses_warmup_output(monkeypatch, capsys):
    prompt_state = PromptRuntimeState()
    calls = []

    class FakePromptScanner:
        def __init__(self, mode):
            calls.append(("init", mode.value))

        def warmup(self):
            print("download progress on stdout")
            print("download progress on stderr", file=sys.stderr)
            calls.append(("warmup",))

        def scan(self, text, source=None):
            calls.append(("scan", text, source))

    monkeypatch.setattr(
        "agent_sec_cli.prompt_scanner.scanner.PromptScanner",
        FakePromptScanner,
    )

    _preload_prompt_model_sync(prompt_state, "strict", "probe")
    captured = capsys.readouterr()

    assert calls == [
        ("init", "strict"),
        ("warmup",),
        ("scan", "probe", "daemon-startup"),
    ]
    assert captured.out == ""
    assert captured.err == ""
    assert prompt_state.model == "LLM-Research/Llama-Prompt-Guard-2-86M"
    assert prompt_state.status == "loading"


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
