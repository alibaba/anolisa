"""Background job lifecycle framework for the daemon."""

import asyncio
import contextlib
import math
from abc import ABC, abstractmethod
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from typing import Any


@dataclass(frozen=True)
class JobStatus:
    """Serializable background job state."""

    name: str
    state: str
    last_error: str | None = None
    last_tick_at: str | None = None
    interval_seconds: float | None = None
    last_started_at: str | None = None
    next_run_at: str | None = None

    def to_dict(self) -> dict[str, Any]:
        """Return a JSON-serializable status payload."""
        payload: dict[str, Any] = {
            "name": self.name,
            "state": self.state,
            "last_error": self.last_error,
            "last_tick_at": self.last_tick_at,
        }
        if self.interval_seconds is not None:
            payload["interval_seconds"] = self.interval_seconds
        if self.last_started_at is not None:
            payload["last_started_at"] = self.last_started_at
        if self.next_run_at is not None:
            payload["next_run_at"] = self.next_run_at
        return payload


class BackgroundJob(ABC):
    """Base class for daemon background jobs."""

    name = "background-job"

    @abstractmethod
    async def start(self) -> None:
        """Start the job."""
        pass

    @abstractmethod
    async def stop(self) -> None:
        """Stop the job."""
        pass

    @abstractmethod
    def status(self) -> JobStatus:
        """Return current job status."""
        pass


class PeriodicBackgroundJob(BackgroundJob, ABC):
    """Background job that runs once per interval boundary.

    Scheduling is anchored to each run start time. If a run takes longer than
    one interval, the scheduler skips missed boundaries and waits for the next
    future interval boundary instead of running back-to-back.
    """

    def __init__(self, interval_seconds: float) -> None:
        if interval_seconds <= 0:
            raise ValueError("interval_seconds must be positive")

        self.interval_seconds = interval_seconds
        self._task: asyncio.Task[None] | None = None
        self._stop_event: asyncio.Event | None = None
        self._state = "stopped"
        self._last_error: str | None = None
        self._last_tick_at: str | None = None
        self._last_started_at: str | None = None
        self._next_run_at: str | None = None

    async def start(self) -> None:
        """Start the periodic loop."""
        if self._task is not None and not self._task.done():
            return

        self._stop_event = asyncio.Event()
        self._state = "running"
        self._task = asyncio.create_task(self._run_loop())

    async def stop(self) -> None:
        """Stop the periodic loop."""
        if self._stop_event is not None:
            self._stop_event.set()

        if self._task is not None:
            self._task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._task
            self._task = None

        self._state = "stopped"
        self._stop_event = None

    def status(self) -> JobStatus:
        """Return current periodic job status."""
        return JobStatus(
            name=self.name,
            state=self._state,
            last_error=self._last_error,
            last_tick_at=self._last_tick_at,
            interval_seconds=self.interval_seconds,
            last_started_at=self._last_started_at,
            next_run_at=self._next_run_at,
        )

    @abstractmethod
    async def run_once(self) -> None:
        """Run one periodic job iteration."""
        pass

    async def _run_loop(self) -> None:
        next_run_monotonic = time_monotonic()
        self._next_run_at = utc_now()

        while self._stop_event is not None and not self._stop_event.is_set():
            await self._wait_until(next_run_monotonic)
            if self._stop_event is None or self._stop_event.is_set():
                break

            started_monotonic = time_monotonic()
            started_at = utc_now()
            self._last_started_at = started_at
            self._last_tick_at = started_at

            try:
                await self.run_once()
                self._last_error = None
                self._state = "running"
            except asyncio.CancelledError:
                raise
            except Exception as exc:
                self._last_error = str(exc)
                self._state = "error"

            finished_monotonic = time_monotonic()
            next_run_monotonic = next_cycle_start(
                started_monotonic,
                finished_monotonic,
                self.interval_seconds,
            )
            wait_seconds = max(0.0, next_run_monotonic - finished_monotonic)
            self._next_run_at = utc_after(wait_seconds)

    async def _wait_until(self, run_at_monotonic: float) -> None:
        wait_seconds = max(0.0, run_at_monotonic - time_monotonic())
        if wait_seconds == 0:
            return
        if self._stop_event is None:
            return

        with contextlib.suppress(asyncio.TimeoutError):
            await asyncio.wait_for(self._stop_event.wait(), timeout=wait_seconds)


class JobManager:
    """Tracks daemon background jobs and exposes their status."""

    def __init__(self) -> None:
        self._jobs: list[BackgroundJob] = []
        self._started = False

    def register(self, job: BackgroundJob) -> None:
        """Register a background job before daemon startup."""
        self._jobs.append(job)

    async def start_all(self) -> None:
        """Start all registered jobs."""
        for job in self._jobs:
            await job.start()
        self._started = True

    async def stop_all(self) -> None:
        """Stop all registered jobs in reverse registration order."""
        for job in reversed(self._jobs):
            await job.stop()
        self._started = False

    def status(self) -> list[dict[str, Any]]:
        """Return JSON-serializable status for all jobs."""
        return [job.status().to_dict() for job in self._jobs]

    @property
    def started(self) -> bool:
        """Return whether the manager has started its jobs."""
        return self._started


def next_cycle_start(
    started_monotonic: float,
    finished_monotonic: float,
    interval_seconds: float,
) -> float:
    """Return the next interval boundary anchored to a run start time."""
    if interval_seconds <= 0:
        raise ValueError("interval_seconds must be positive")

    elapsed = max(0.0, finished_monotonic - started_monotonic)
    cycle_index = max(1, math.ceil(elapsed / interval_seconds))
    return started_monotonic + (cycle_index * interval_seconds)


def time_monotonic() -> float:
    """Return monotonic time for periodic scheduling."""
    return asyncio.get_running_loop().time()


def utc_now() -> str:
    """Return the current UTC timestamp for job status."""
    return _format_utc(datetime.now(timezone.utc))


def utc_after(seconds: float) -> str:
    """Return a UTC timestamp approximately seconds in the future."""
    return _format_utc(datetime.now(timezone.utc) + timedelta(seconds=seconds))


def _format_utc(value: datetime) -> str:
    return value.isoformat().replace("+00:00", "Z")
