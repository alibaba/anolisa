"""Tests for the SkillFS daemon notification handler."""

# ruff: noqa: I001

import asyncio
from pathlib import Path
from typing import Any

import pytest
from agent_sec_cli.daemon.errors import BadRequestError
from agent_sec_cli.daemon.handlers.skill_ledger import (
    METHOD_SKILLFS_NOTIFY_CHANGE,
    parse_skillfs_change,
    register_skill_ledger_methods,
    skillfs_notify_change_handler,
)
from agent_sec_cli.daemon.jobs.skill_ledger import SkillLedgerActivationJob
from agent_sec_cli.daemon.jobs.skill_ledger.protocol import SkillFsChange
from agent_sec_cli.daemon.protocol import DaemonRequest
from agent_sec_cli.daemon.registry import MethodRegistry
from agent_sec_cli.daemon.runtime import DaemonRuntime


class FakeWorkerClient:
    """In-process worker client for handler queueing tests."""

    def __init__(self) -> None:
        self.last_error = None
        self.pid = None

    async def process_change(self, change: SkillFsChange) -> dict[str, Any]:
        return {"status": "processed", "skill": change.to_dict()}

    async def stop(self) -> None:
        pass


def make_skill(tmp_path: Path, name: str = "demo-skill") -> Path:
    """Create a minimal skill directory for daemon tests."""
    skill_dir = tmp_path / name
    skill_dir.mkdir()
    (skill_dir / "SKILL.md").write_text("# Demo Skill\n", encoding="utf-8")
    return skill_dir


def request_for(skill_dir: Path, **overrides: Any) -> DaemonRequest:
    """Build a daemon request for SkillFS notify tests."""
    params: dict[str, Any] = {
        "schemaVersion": 1,
        "skillDir": str(skill_dir),
        "skillName": skill_dir.name,
        "eventKind": "write",
        "paths": ["SKILL.md"],
    }
    params.update(overrides)
    return DaemonRequest(
        method=METHOD_SKILLFS_NOTIFY_CHANGE,
        params=params,
    )


def test_register_skill_ledger_methods():
    registry = MethodRegistry()

    register_skill_ledger_methods(registry)

    spec = registry.get(METHOD_SKILLFS_NOTIFY_CHANGE)
    assert spec.handler is skillfs_notify_change_handler
    assert spec.queue == "skill_ledger"


def test_parse_skillfs_change_validates_request(tmp_path: Path):
    skill_dir = make_skill(tmp_path, "weather")

    change = parse_skillfs_change(request_for(skill_dir).params)

    assert change.skill_dir == skill_dir.resolve()
    assert change.skill_name == "weather"
    assert change.event_kinds == {"write"}
    assert change.paths == {"SKILL.md"}


def test_parse_skillfs_change_accepts_reconcile_with_empty_paths(tmp_path: Path):
    skill_dir = make_skill(tmp_path, "weather")

    change = parse_skillfs_change(
        request_for(skill_dir, eventKind="reconcile", paths=[]).params
    )

    assert change.skill_dir == skill_dir.resolve()
    assert change.skill_name == "weather"
    assert change.event_kinds == {"reconcile"}
    assert change.paths == set()


@pytest.mark.parametrize(
    ("overrides", "message"),
    [
        ({"schemaVersion": 2}, "schemaVersion"),
        ({"skillDir": "relative-skill"}, "absolute path"),
        ({"skillDir": "~/relative-to-home"}, "absolute path"),
        ({"skillName": "other"}, "skillName"),
        ({"eventKind": "chmod"}, "eventKind"),
        ({"paths": "/absolute"}, "paths"),
        ({"paths": ["/absolute"]}, "relative paths"),
        ({"paths": ["../escape"]}, "relative paths"),
        ({"paths": ["."]}, "relative paths"),
        ({"paths": ["scripts/run\x00.sh"]}, "NUL"),
    ],
)
def test_parse_skillfs_change_rejects_invalid_params(
    tmp_path: Path,
    overrides: dict[str, Any],
    message: str,
):
    skill_dir = make_skill(tmp_path, "weather")

    with pytest.raises(BadRequestError, match=message):
        parse_skillfs_change(request_for(skill_dir, **overrides).params)


def test_parse_skillfs_change_rejects_nul_skill_dir(tmp_path: Path):
    skill_dir = make_skill(tmp_path, "weather")

    with pytest.raises(BadRequestError, match="NUL"):
        parse_skillfs_change(request_for(skill_dir, skillDir=f"{skill_dir}\x00").params)


def test_parse_skillfs_change_requires_skill_manifest(tmp_path: Path):
    skill_dir = tmp_path / "not-a-skill"
    skill_dir.mkdir()

    with pytest.raises(BadRequestError, match="SKILL.md"):
        parse_skillfs_change(request_for(skill_dir).params)


def test_metadata_only_notification_is_accepted_and_ignored(tmp_path: Path):
    skill_dir = make_skill(tmp_path, "weather")
    runtime = DaemonRuntime(socket_path=tmp_path / "daemon.sock")

    response = skillfs_notify_change_handler(
        request_for(skill_dir, paths=[".skill-meta/latest.json"]),
        runtime,
    )

    assert response.data["schemaVersion"] == 1
    assert response.data["accepted"] is True
    assert response.data["ignored"] is True
    assert response.data["reason"] == "metadata-only change"


def test_notify_enqueues_registered_activation_job(monkeypatch, tmp_path: Path):
    skill_dir = make_skill(tmp_path, "weather")
    runtime = DaemonRuntime(socket_path=tmp_path / "daemon.sock")
    job = SkillLedgerActivationJob(
        debounce_seconds=0,
        worker_client=FakeWorkerClient(),
    )
    runtime.jobs.register(job)
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.activation._resolve_managed_skill_dirs",
        lambda: [],
    )

    async def scenario():
        await job.start()
        try:
            response = skillfs_notify_change_handler(request_for(skill_dir), runtime)
        finally:
            await job.stop()
        return response

    response = asyncio.run(scenario())

    assert response.data["schemaVersion"] == 1
    assert response.data["accepted"] is True
    assert response.data["ignored"] is False
    assert response.data["queued"] is True
    assert response.data["coalesced"] is False
    assert response.data["skill"]["skillName"] == "weather"


def test_notify_enqueues_reconcile_with_empty_paths(monkeypatch, tmp_path: Path):
    skill_dir = make_skill(tmp_path, "weather")
    runtime = DaemonRuntime(socket_path=tmp_path / "daemon.sock")
    job = SkillLedgerActivationJob(
        debounce_seconds=0,
        worker_client=FakeWorkerClient(),
    )
    runtime.jobs.register(job)
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.activation._resolve_managed_skill_dirs",
        lambda: [],
    )

    async def scenario():
        await job.start()
        try:
            response = skillfs_notify_change_handler(
                request_for(skill_dir, eventKind="reconcile", paths=[]),
                runtime,
            )
        finally:
            await job.stop()
        return response

    response = asyncio.run(scenario())

    assert response.data["schemaVersion"] == 1
    assert response.data["accepted"] is True
    assert response.data["ignored"] is False
    assert response.data["queued"] is True
    assert response.data["coalesced"] is False
    assert response.data["skill"]["eventKinds"] == ["reconcile"]
    assert response.data["skill"]["paths"] == []
