"""Tests for daemon background job scheduling."""

import asyncio

import pytest
from agent_sec_cli.daemon.jobs import (
    JobStatus,
    PeriodicBackgroundJob,
    next_cycle_start,
)


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
