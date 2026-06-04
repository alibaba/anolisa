"""Prompt scanner model preload background job."""

import asyncio
import contextlib
import os
import sys
from typing import Any

from agent_sec_cli.daemon.jobs.base import BackgroundJob, JobStatus, utc_now

PROMPT_PRELOAD_ENV = "AGENT_SEC_DAEMON_PROMPT_PRELOAD"
PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_ENV = (
    "AGENT_SEC_DAEMON_PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_SECONDS"
)
PROMPT_PRELOAD_JOB_NAME = "prompt-model-preload"
PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_SECONDS = 600.0
PROMPT_PRELOAD_CHILD_TERMINATE_TIMEOUT_SECONDS = 5.0
_PROMPT_PRELOAD_CHILD_CODE = """
import sys

from agent_sec_cli.daemon.jobs.prompt_preload import _download_prompt_model_sync


try:
    _download_prompt_model_sync(sys.argv[1])
except Exception as exc:
    print(str(exc), file=sys.stderr)
    raise SystemExit(1)
""".strip()


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
            await _run_preload_child_process(self._mode)
            _update_prompt_state(self._prompt_state, status="loading")
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


async def _run_preload_child_process(mode: str) -> None:
    """Run preload once in a child process so startup downloads are killable."""
    download_timeout_seconds = _prompt_preload_download_timeout_seconds()
    process = await asyncio.create_subprocess_exec(
        sys.executable,
        "-c",
        _PROMPT_PRELOAD_CHILD_CODE,
        mode,
        stdout=asyncio.subprocess.DEVNULL,
        stderr=asyncio.subprocess.PIPE,
    )

    try:
        _stdout, stderr = await asyncio.wait_for(
            process.communicate(),
            timeout=download_timeout_seconds,
        )
    except asyncio.TimeoutError as exc:
        await _terminate_child_process(process)
        raise RuntimeError(
            "prompt preload child timed out after " f"{download_timeout_seconds:g}s"
        ) from exc
    except asyncio.CancelledError:
        await _terminate_child_process(process)
        raise

    if process.returncode == 0:
        return

    stderr_text = stderr.decode("utf-8", errors="replace").strip() if stderr else ""
    if not stderr_text:
        stderr_text = (
            f"prompt preload child process exited with code {process.returncode}"
        )
    raise RuntimeError(stderr_text)


def _prompt_preload_download_timeout_seconds() -> float:
    raw_value = os.environ.get(PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_ENV)
    if raw_value is None:
        return PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_SECONDS

    try:
        timeout_seconds = float(raw_value)
    except ValueError:
        return PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_SECONDS

    if timeout_seconds <= 0:
        return PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_SECONDS
    return timeout_seconds


async def _terminate_child_process(process: asyncio.subprocess.Process) -> None:
    if process.returncode is not None:
        return

    with contextlib.suppress(ProcessLookupError):
        process.terminate()

    try:
        await asyncio.wait_for(
            process.wait(),
            timeout=PROMPT_PRELOAD_CHILD_TERMINATE_TIMEOUT_SECONDS,
        )
    except asyncio.TimeoutError:
        with contextlib.suppress(ProcessLookupError):
            process.kill()
        await process.wait()


def _download_prompt_model_sync(mode: str) -> None:
    """Download prompt model files without loading them into daemon memory."""
    from agent_sec_cli.prompt_scanner.config import (  # noqa: PLC0415 - lazy import: daemon preload only
        ScanMode,
    )
    from agent_sec_cli.prompt_scanner.scanner import (  # noqa: PLC0415 - lazy import: daemon preload only
        PromptScanner,
    )

    scanner = PromptScanner(mode=ScanMode(mode))
    _warmup_silently(scanner)


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
