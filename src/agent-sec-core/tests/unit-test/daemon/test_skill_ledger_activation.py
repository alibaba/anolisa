"""Tests for Skill Ledger activation daemon integration."""

# ruff: noqa: I001

import asyncio
from pathlib import Path
from typing import Any

import pytest
from agent_sec_cli.daemon.jobs.skill_ledger import (
    SKILL_LEDGER_ACTIVATION_JOB,
    SkillLedgerActivationJob,
)
from agent_sec_cli.daemon.jobs.skill_ledger.processor import (
    process_skill_change,
)
from agent_sec_cli.daemon.jobs.skill_ledger.protocol import SkillFsChange
from agent_sec_cli.daemon.jobs.skill_ledger.worker_client import (
    SkillLedgerWorkerTransportError,
)
from agent_sec_cli.skill_ledger.errors import UnresolvedLiveRootError


class FakeWorkerClient:
    """In-process worker client for activation scheduling tests."""

    def __init__(self, process=None):
        self._process = process or (
            lambda change: {"status": "processed", "skill": change.to_dict()}
        )
        self.last_error = None
        self.pid = None
        self.stopped = False

    async def process_change(self, change: SkillFsChange) -> dict[str, Any]:
        return self._process(change)

    async def stop(self) -> None:
        self.stopped = True


def make_skill(tmp_path: Path, name: str = "demo-skill") -> Path:
    """Create a minimal skill directory for daemon tests."""
    skill_dir = tmp_path / name
    skill_dir.mkdir()
    (skill_dir / "SKILL.md").write_text("# Demo Skill\n", encoding="utf-8")
    return skill_dir


def test_activation_job_debounces_same_skill(monkeypatch, tmp_path: Path):
    skill_dir = make_skill(tmp_path, "weather")
    calls = []

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.activation._resolve_managed_skill_dirs",
        lambda: [],
    )

    def fake_process(change: SkillFsChange) -> dict[str, Any]:
        calls.append(change)
        return {"status": "processed", "skill": change.to_dict()}

    async def scenario():
        job = SkillLedgerActivationJob(
            debounce_seconds=0.05,
            worker_client=FakeWorkerClient(fake_process),
        )
        await job.start()
        try:
            job.enqueue(
                SkillFsChange(
                    skill_dir=skill_dir.resolve(),
                    skill_name=skill_dir.name,
                    event_kinds={"write"},
                    paths={"SKILL.md"},
                )
            )
            job.enqueue(
                SkillFsChange(
                    skill_dir=skill_dir.resolve(),
                    skill_name=skill_dir.name,
                    event_kinds={"rename"},
                    paths={"scripts/run.sh"},
                )
            )
            job.enqueue(
                SkillFsChange(
                    skill_dir=skill_dir.resolve(),
                    skill_name=skill_dir.name,
                    event_kinds={"reconcile"},
                    paths=set(),
                )
            )
            deadline = asyncio.get_running_loop().time() + 1.0
            while len(calls) < 1 and asyncio.get_running_loop().time() < deadline:
                await asyncio.sleep(0.01)
        finally:
            await job.stop()

    asyncio.run(scenario())

    assert len(calls) == 1
    assert calls[0].event_kinds == {"write", "rename", "reconcile"}
    assert calls[0].paths == {"SKILL.md", "scripts/run.sh"}


def test_activation_job_debounces_events_arriving_during_drain(
    monkeypatch,
    tmp_path: Path,
):
    skill_dir = make_skill(tmp_path, "weather")
    calls: list[tuple[set[str], float]] = []

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.activation._resolve_managed_skill_dirs",
        lambda: [],
    )

    async def scenario():
        job = SkillLedgerActivationJob(debounce_seconds=0.05)

        async def fake_process(change: SkillFsChange) -> None:
            calls.append((set(change.event_kinds), asyncio.get_running_loop().time()))
            if len(calls) == 1:
                job.enqueue(
                    SkillFsChange(
                        skill_dir=skill_dir.resolve(),
                        skill_name=skill_dir.name,
                        event_kinds={"rename"},
                        paths={"scripts/run.sh"},
                    )
                )

        monkeypatch.setattr(job, "_process_change", fake_process)
        await job.start()
        try:
            job.enqueue(
                SkillFsChange(
                    skill_dir=skill_dir.resolve(),
                    skill_name=skill_dir.name,
                    event_kinds={"write"},
                    paths={"SKILL.md"},
                )
            )
            deadline = asyncio.get_running_loop().time() + 1.0
            while len(calls) < 2 and asyncio.get_running_loop().time() < deadline:
                await asyncio.sleep(0.01)
        finally:
            await job.stop()

    asyncio.run(scenario())

    assert [event_kinds for event_kinds, _ in calls] == [{"write"}, {"rename"}]
    assert calls[1][1] - calls[0][1] >= 0.04


def test_drain_pending_requeues_batch_on_cancelled_process(
    monkeypatch,
    tmp_path: Path,
):
    first = make_skill(tmp_path, "weather")
    second = make_skill(tmp_path, "calendar")

    async def scenario():
        job = SkillLedgerActivationJob(debounce_seconds=0)
        job._wake_event = asyncio.Event()
        changes = [
            SkillFsChange(
                skill_dir=first.resolve(),
                skill_name=first.name,
                event_kinds={"write"},
                paths={"SKILL.md"},
            ),
            SkillFsChange(
                skill_dir=second.resolve(),
                skill_name=second.name,
                event_kinds={"write"},
                paths={"SKILL.md"},
            ),
        ]
        job._pending = {change.skill_dir: change for change in changes}

        async def fail_process(_change: SkillFsChange) -> None:
            raise asyncio.CancelledError()

        monkeypatch.setattr(job, "_process_change", fail_process)
        with pytest.raises(asyncio.CancelledError):
            await job._drain_pending()
        return job._pending

    pending = asyncio.run(scenario())

    assert set(pending) == {first.resolve(), second.resolve()}


def test_activation_job_records_worker_transport_failure(tmp_path: Path):
    skill_dir = make_skill(tmp_path, "weather")

    def fail_process(_change: SkillFsChange) -> dict[str, Any]:
        raise SkillLedgerWorkerTransportError("worker request timed out after 300s")

    async def scenario():
        job = SkillLedgerActivationJob(
            debounce_seconds=0,
            worker_client=FakeWorkerClient(fail_process),
        )
        job._state = "running"
        await job._process_change(
            SkillFsChange(
                skill_dir=skill_dir.resolve(),
                skill_name=skill_dir.name,
                event_kinds={"write"},
                paths={"SKILL.md"},
            )
        )
        return job.status(), job.last_processed

    status, last_processed = asyncio.run(scenario())

    assert status.state == "error"
    assert status.last_error == "worker request timed out after 300s"
    assert last_processed is not None
    assert last_processed["status"] == "error"


def test_process_skill_change_resolves_activation_after_scan_error(
    monkeypatch,
    tmp_path: Path,
):
    skill_dir = make_skill(tmp_path, "weather")
    backend = object()
    events = []

    def fake_backend() -> object:
        return backend

    def fail_scan(path: str, received_backend: object) -> dict[str, Any]:
        events.append(("scan", path, received_backend))
        raise RuntimeError("scanner failed")

    def fake_policy() -> str:
        return "pass_only"

    def fake_resolve(
        path: str,
        received_backend: object,
        policy: str,
    ) -> dict[str, Any]:
        events.append(("resolve", path, received_backend, policy))
        return {"target": None}

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._ensure_default_backend",
        fake_backend,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._scan_skill",
        fail_scan,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._resolve_activation",
        fake_resolve,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._resolve_activation_policy",
        fake_policy,
    )

    result = process_skill_change(
        SkillFsChange(
            skill_dir=skill_dir.resolve(),
            skill_name=skill_dir.name,
            event_kinds={"write"},
            paths={"SKILL.md"},
        )
    )

    assert result["status"] == "error"
    assert result["error"] == "scanner failed"
    assert result["activation"] == {"target": None}
    assert events == [
        ("scan", str(skill_dir.resolve()), backend),
        ("resolve", str(skill_dir.resolve()), backend, "pass_only"),
    ]


def test_process_skill_change_skips_unresolved_live_root_from_scan(
    monkeypatch,
    tmp_path: Path,
):
    skill_dir = make_skill(tmp_path, "weather")
    backend = object()
    live_root_error = UnresolvedLiveRootError(skill_dir.resolve())

    def fake_backend() -> object:
        return backend

    def fail_scan(path: str, received_backend: object) -> dict[str, Any]:
        assert path == str(skill_dir.resolve())
        assert received_backend is backend
        raise live_root_error

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._ensure_default_backend",
        fake_backend,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._scan_skill",
        fail_scan,
    )

    result = process_skill_change(
        SkillFsChange(
            skill_dir=skill_dir.resolve(),
            skill_name=skill_dir.name,
            event_kinds={"write"},
            paths={"SKILL.md"},
        )
    )

    assert result["status"] == "skipped"
    assert result["reasonCode"] == "unmanaged_skill_root"
    assert result["message"] == str(live_root_error)
    assert result["skill"]["skillName"] == "weather"
    assert result["scan"] is None
    assert result["activation"] is None
    assert "error" not in result


def test_process_skill_change_skips_unresolved_live_root_from_activation(
    monkeypatch,
    tmp_path: Path,
):
    skill_dir = make_skill(tmp_path, "weather")
    backend = object()
    live_root_error = UnresolvedLiveRootError(skill_dir.resolve())

    def fake_backend() -> object:
        return backend

    def fake_scan(path: str, received_backend: object) -> dict[str, Any]:
        assert path == str(skill_dir.resolve())
        assert received_backend is backend
        return {"status": "noop"}

    def fail_resolve(
        path: str, received_backend: object, policy: str
    ) -> dict[str, Any]:
        assert path == str(skill_dir.resolve())
        assert received_backend is backend
        assert policy == "pass_only"
        raise live_root_error

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._ensure_default_backend",
        fake_backend,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._scan_skill",
        fake_scan,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._resolve_activation",
        fail_resolve,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._resolve_activation_policy",
        lambda: "pass_only",
    )

    result = process_skill_change(
        SkillFsChange(
            skill_dir=skill_dir.resolve(),
            skill_name=skill_dir.name,
            event_kinds={"write"},
            paths={"SKILL.md"},
        )
    )

    assert result["status"] == "skipped"
    assert result["reasonCode"] == "unmanaged_skill_root"
    assert result["message"] == str(live_root_error)
    assert result["skill"]["skillName"] == "weather"
    assert result["scan"] is None
    assert result["activation"] is None
    assert "error" not in result


def test_process_skill_change_does_not_skip_live_root_after_scan_error(
    monkeypatch,
    tmp_path: Path,
):
    skill_dir = make_skill(tmp_path, "weather")
    backend = object()
    scan_failure = RuntimeError("scanner failed")
    live_root_error = UnresolvedLiveRootError(skill_dir.resolve())

    def fake_backend() -> object:
        return backend

    def fail_scan(path: str, received_backend: object) -> dict[str, Any]:
        assert path == str(skill_dir.resolve())
        assert received_backend is backend
        raise scan_failure

    def fail_resolve(
        path: str, received_backend: object, policy: str
    ) -> dict[str, Any]:
        assert path == str(skill_dir.resolve())
        assert received_backend is backend
        assert policy == "pass_only"
        raise live_root_error

    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._ensure_default_backend",
        fake_backend,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._scan_skill",
        fail_scan,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._resolve_activation",
        fail_resolve,
    )
    monkeypatch.setattr(
        "agent_sec_cli.daemon.jobs.skill_ledger.processor._resolve_activation_policy",
        lambda: "pass_only",
    )

    with pytest.raises(UnresolvedLiveRootError) as exc_info:
        process_skill_change(
            SkillFsChange(
                skill_dir=skill_dir.resolve(),
                skill_name=skill_dir.name,
                event_kinds={"write"},
                paths={"SKILL.md"},
            )
        )

    assert exc_info.value is live_root_error
    assert exc_info.value.__cause__ is scan_failure


def test_default_job_name_is_stable():
    assert SkillLedgerActivationJob().name == SKILL_LEDGER_ACTIVATION_JOB
