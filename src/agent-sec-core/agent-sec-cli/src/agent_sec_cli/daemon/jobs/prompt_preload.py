"""Prompt scanner model preload background job."""

import asyncio
import contextlib
import os
import sys
from typing import Any

from agent_sec_cli.daemon.jobs.base import OneShotBackgroundJob

PROMPT_PRELOAD_ENV = "AGENT_SEC_DAEMON_PROMPT_PRELOAD"
PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_ENV = (
    "AGENT_SEC_DAEMON_PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_SECONDS"
)
PROMPT_PRELOAD_JOB_NAME = "prompt-model-preload"
PROMPT_PRELOAD_DOWNLOAD_TIMEOUT_SECONDS = 600.0
PROMPT_PRELOAD_CHILD_TERMINATE_TIMEOUT_SECONDS = 5.0
_PROMPT_PRELOAD_CHILD_MODULE = "agent_sec_cli.daemon.jobs.prompt_preload"


class PromptModelPreloadJob(OneShotBackgroundJob):
    """One-shot startup job that loads and probes the prompt model."""

    name = PROMPT_PRELOAD_JOB_NAME

    def __init__(
        self,
        prompt_state: Any,
        mode: str = "strict",
        probe_text: str = "hello",
    ) -> None:
        super().__init__()
        self._prompt_state = prompt_state
        self._mode = mode
        self._probe_text = probe_text

    def on_run_started(self, started_at: str) -> None:
        """Mark prompt model preload as loading from the local cache."""
        _update_prompt_state(
            self._prompt_state,
            status="loading",
            loaded=False,
            last_error=None,
            last_started_at=started_at,
            last_finished_at=None,
        )

    async def run_once(self) -> None:
        """Load and probe locally, downloading once only when loading fails."""
        try:
            await self._load_and_probe()
        except Exception:
            # Preserve the previous recovery contract with one bounded download/repair attempt.
            _update_prompt_state(self._prompt_state, status="downloading")
            await _run_preload_child_process(self._mode)
            _update_prompt_state(self._prompt_state, status="loading")
            await self._load_and_probe()

    async def _load_and_probe(self) -> None:
        """Load and probe the prompt model without blocking the event loop."""
        await asyncio.to_thread(
            _preload_prompt_model_sync,
            self._prompt_state,
            self._mode,
            self._probe_text,
        )

    def on_run_cancelled(self, finished_at: str) -> None:
        """Mark prompt model preload as stopped after cancellation."""
        _update_prompt_state(
            self._prompt_state,
            status="stopped",
            loaded=False,
            last_error=None,
            last_finished_at=finished_at,
        )

    def on_run_failed(self, exc: Exception, finished_at: str) -> None:
        """Mark prompt model preload as degraded after failure."""
        _update_prompt_state(
            self._prompt_state,
            status="degraded",
            loaded=False,
            last_error=str(exc),
            last_finished_at=finished_at,
        )

    def on_run_completed(self, finished_at: str) -> None:
        """Mark prompt model preload as ready after successful preload."""
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
        "-m",
        _PROMPT_PRELOAD_CHILD_MODULE,
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
    """Load and probe the prompt model in a worker thread.

    The first call uses only the local cache.  The job may run a download-only
    child and call this function once more if local model loading fails.
    Avoid redirecting sys.stdout/sys.stderr here: those are process-global
    objects, so changing them in this worker thread could hide unrelated
    daemon output from other threads.
    """
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
    result = scanner.scan(probe_text, source="daemon-startup")

    if not any(layer.layer_name == "ml_classifier" for layer in result.layer_results):
        raise RuntimeError("prompt model probe did not execute the ML classifier")


def _warmup_silently(scanner: Any) -> None:
    """Run child-process warmup without writing download progress to stdio."""
    with open(os.devnull, "w") as devnull, contextlib.redirect_stdout(
        devnull
    ), contextlib.redirect_stderr(devnull):
        scanner.warmup()


def _update_prompt_state(prompt_state: Any, **updates: Any) -> None:
    for field_name, value in updates.items():
        setattr(prompt_state, field_name, value)


def _main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    if len(args) != 1:
        print(
            "usage: python -m agent_sec_cli.daemon.jobs.prompt_preload <mode>",
            file=sys.stderr,
        )
        return 2

    try:
        _download_prompt_model_sync(args[0])
    except Exception as exc:
        print(str(exc), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
