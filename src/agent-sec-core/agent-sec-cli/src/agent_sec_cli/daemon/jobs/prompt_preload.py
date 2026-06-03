"""Prompt scanner model preload background job."""

import asyncio
import contextlib
import os
from typing import Any

from agent_sec_cli.daemon.jobs.base import BackgroundJob, JobStatus, utc_now

PROMPT_PRELOAD_ENV = "AGENT_SEC_DAEMON_PROMPT_PRELOAD"
PROMPT_PRELOAD_JOB_NAME = "prompt-model-preload"


class PromptModelPreloadJob(BackgroundJob):
    """One-shot startup job that downloads, loads, and probes the prompt model."""

    name = PROMPT_PRELOAD_JOB_NAME

    def __init__(
        self,
        prompt_state: Any,
        mode: str = "strict",
        probe_text: str = "hello",
    ) -> None:
        self._prompt_state = prompt_state
        self._mode = mode
        self._probe_text = probe_text
        self._task: asyncio.Task[None] | None = None
        self._state = "stopped"
        self._last_error: str | None = None
        self._last_tick_at: str | None = None
        self._last_started_at: str | None = None

    async def start(self) -> None:
        """Start prompt model preload without blocking daemon startup."""
        if self._task is not None and not self._task.done():
            return

        self._state = "running"
        self._task = asyncio.create_task(self._run_once())

    async def stop(self) -> None:
        """Cancel the preload task if it has not completed."""
        if self._task is not None and not self._task.done():
            self._task.cancel()
            with contextlib.suppress(asyncio.CancelledError):
                await self._task

        if self._state == "running":
            self._state = "stopped"
        self._task = None

    def status(self) -> JobStatus:
        """Return current prompt preload job status."""
        return JobStatus(
            name=self.name,
            state=self._state,
            last_error=self._last_error,
            last_tick_at=self._last_tick_at,
            last_started_at=self._last_started_at,
        )

    async def _run_once(self) -> None:
        started_at = utc_now()
        self._last_started_at = started_at
        self._last_tick_at = started_at
        _update_prompt_state(
            self._prompt_state,
            status="downloading",
            loaded=False,
            last_error=None,
            last_started_at=started_at,
            last_finished_at=None,
        )

        try:
            await asyncio.to_thread(
                _preload_prompt_model_sync,
                self._prompt_state,
                self._mode,
                self._probe_text,
            )
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            finished_at = utc_now()
            self._last_error = str(exc)
            self._state = "error"
            _update_prompt_state(
                self._prompt_state,
                status="degraded",
                loaded=False,
                last_error=self._last_error,
                last_finished_at=finished_at,
            )
            return

        finished_at = utc_now()
        self._last_error = None
        self._state = "completed"
        _update_prompt_state(
            self._prompt_state,
            status="ready",
            loaded=True,
            last_error=None,
            last_finished_at=finished_at,
        )


def prompt_preload_enabled() -> bool:
    """Return whether daemon startup should trigger prompt model preload."""
    raw_value = os.environ.get(PROMPT_PRELOAD_ENV, "1").strip().lower()
    return raw_value not in {"0", "false", "no", "off"}


def _preload_prompt_model_sync(
    prompt_state: Any,
    mode: str,
    probe_text: str,
) -> None:
    """Download, load, and probe the prompt model in a worker thread."""
    from agent_sec_cli.prompt_scanner.config import (  # noqa: PLC0415 - lazy import: daemon preload only
        ScanMode,
        get_config,
    )
    from agent_sec_cli.prompt_scanner.scanner import (  # noqa: PLC0415 - lazy import: daemon preload only
        PromptScanner,
    )

    scan_mode = ScanMode(mode)
    config = get_config(scan_mode)
    _update_prompt_state(prompt_state, model=config.model_name)

    scanner = PromptScanner(mode=scan_mode)
    _warmup_silently(scanner)
    _update_prompt_state(prompt_state, status="loading")
    scanner.scan(probe_text, source="daemon-startup")


def _warmup_silently(scanner: Any) -> None:
    """Run daemon-owned warmup without writing download progress to stdio."""
    with open(os.devnull, "w") as devnull, contextlib.redirect_stdout(
        devnull
    ), contextlib.redirect_stderr(devnull):
        scanner.warmup()


def _update_prompt_state(prompt_state: Any, **updates: Any) -> None:
    for field_name, value in updates.items():
        setattr(prompt_state, field_name, value)
