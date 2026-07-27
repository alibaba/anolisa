"""Default daemon background job registration."""

from typing import Any

from agent_sec_cli.daemon.jobs.base import JobManager
from agent_sec_cli.daemon.jobs.prompt_preload import (
    PromptModelPreloadJob,
    prompt_preload_enabled,
)
from agent_sec_cli.daemon.jobs.skill_ledger import (
    SkillLedgerActivationJob,
)


def register_default_jobs(
    job_manager: JobManager,
    prompt_scan_state: Any | None = None,
) -> None:
    """Register daemon jobs that should start with every daemon instance.

    Concrete jobs live in this package as separate modules. Keep this file as
    the central startup registry so daemon startup order stays explicit.
    """
    job_manager.register(SkillLedgerActivationJob())
    if prompt_scan_state is not None and prompt_preload_enabled():
        job_manager.register(PromptModelPreloadJob(prompt_scan_state))
