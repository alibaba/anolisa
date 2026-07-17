"""Daemon handler for SkillFS change notifications."""

from pathlib import Path
from typing import Any

from agent_sec_cli.daemon.errors import BadRequestError, UnavailableError
from agent_sec_cli.daemon.jobs.skill_ledger.activation import (
    SKILL_LEDGER_ACTIVATION_JOB,
)
from agent_sec_cli.daemon.jobs.skill_ledger.protocol import (
    SKILLFS_EVENT_KINDS,
    SkillFsChange,
)
from agent_sec_cli.daemon.protocol import DaemonRequest
from agent_sec_cli.daemon.registry import (
    HandlerResult,
    MethodRegistry,
    MethodSpec,
)
from agent_sec_cli.daemon.runtime import DaemonRuntime

METHOD_SKILLFS_NOTIFY_CHANGE = "skill_ledger.skillfs_notify_change"
SCHEMA_VERSION = 1

_SKILL_MANIFEST = "SKILL.md"
_SKILL_META = ".skill-meta"


def register_skill_ledger_methods(registry: MethodRegistry) -> None:
    """Register the SkillFS notification method."""
    registry.register(
        MethodSpec(
            method=METHOD_SKILLFS_NOTIFY_CHANGE,
            handler=skillfs_notify_change_handler,
            lifecycle="skill_ledger",
            queue="skill_ledger",
            timeout_ms=1000,
            access_log=True,
        )
    )


def skillfs_notify_change_handler(
    request: DaemonRequest,
    runtime: DaemonRuntime,
) -> HandlerResult:
    """Validate a SkillFS change notification and enqueue daemon processing."""
    change = parse_skillfs_change(request.params)
    if _paths_are_metadata_only(change.paths):
        return HandlerResult(
            data={
                "schemaVersion": SCHEMA_VERSION,
                "accepted": True,
                "ignored": True,
                "reason": "metadata-only change",
                "skill": change.to_dict(),
            }
        )

    job = runtime.jobs.get(SKILL_LEDGER_ACTIVATION_JOB)
    if job is None or not hasattr(job, "enqueue"):
        raise UnavailableError("skill-ledger activation job is not registered")
    newly_queued = job.enqueue(change)
    return HandlerResult(
        data={
            "schemaVersion": SCHEMA_VERSION,
            "accepted": True,
            "ignored": False,
            "queued": True,
            "coalesced": not newly_queued,
            "skill": change.to_dict(),
        }
    )


def parse_skillfs_change(params: dict[str, Any]) -> SkillFsChange:
    """Validate daemon request params for a SkillFS change notification."""
    schema_version = params.get("schemaVersion")
    if schema_version != SCHEMA_VERSION:
        raise BadRequestError("params.schemaVersion must be 1")

    skill_dir = _validate_skill_dir(params.get("skillDir"))
    skill_name = params.get("skillName")
    if not isinstance(skill_name, str) or not skill_name:
        raise BadRequestError("params.skillName must be a non-empty string")
    if skill_name != skill_dir.name:
        raise BadRequestError("params.skillName must match skillDir basename")

    event_kind = params.get("eventKind")
    if event_kind not in SKILLFS_EVENT_KINDS:
        allowed = ", ".join(sorted(SKILLFS_EVENT_KINDS))
        raise BadRequestError(f"params.eventKind must be one of: {allowed}")

    paths = _validate_paths(params.get("paths"))
    return SkillFsChange(
        skill_dir=skill_dir,
        skill_name=skill_name,
        event_kinds={event_kind},
        paths=set(paths),
    )


def _validate_skill_dir(value: Any) -> Path:
    if not isinstance(value, str) or not value:
        raise BadRequestError("params.skillDir must be a non-empty string")
    if "\x00" in value:
        raise BadRequestError("params.skillDir must not contain NUL characters")
    path = Path(value)
    if not path.is_absolute():
        raise BadRequestError("params.skillDir must be an absolute path")
    try:
        resolved = path.resolve(strict=True)
    except OSError as exc:
        raise BadRequestError(f"params.skillDir is not accessible: {exc}") from exc
    if not resolved.is_dir():
        raise BadRequestError("params.skillDir must be a directory")
    if not (resolved / _SKILL_MANIFEST).is_file():
        raise BadRequestError("params.skillDir must contain SKILL.md")
    return resolved


def _validate_paths(value: Any) -> list[str]:
    if not isinstance(value, list):
        raise BadRequestError("params.paths must be a list")
    paths: list[str] = []
    for item in value:
        if not isinstance(item, str) or not item:
            raise BadRequestError("params.paths must contain non-empty strings")
        if "\x00" in item:
            raise BadRequestError("params.paths must not contain NUL characters")
        path = Path(item)
        if not path.parts or path.is_absolute() or ".." in path.parts:
            raise BadRequestError("params.paths must be relative paths under skillDir")
        paths.append(item)
    return paths


def _paths_are_metadata_only(paths: set[str]) -> bool:
    return bool(paths) and all(Path(path).parts[0] == _SKILL_META for path in paths)
