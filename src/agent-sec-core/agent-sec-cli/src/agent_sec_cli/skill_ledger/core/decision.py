"""User decisions for Skill Ledger runtime exposure."""

from __future__ import annotations

import fcntl
import json
import shutil
from contextlib import contextmanager
from pathlib import Path
from typing import Any

from agent_sec_cli.skill_ledger.config import resolve_activation_policy
from agent_sec_cli.skill_ledger.core.certifier import _sign_manifest, scan_skill
from agent_sec_cli.skill_ledger.core.checker import check
from agent_sec_cli.skill_ledger.core.exposure import build_exposure_summary
from agent_sec_cli.skill_ledger.core.file_hasher import (
    compute_file_hashes,
    diff_file_hashes,
)
from agent_sec_cli.skill_ledger.core.manifest_helpers import (
    safe_load_latest_manifest,
    snapshot_matches_manifest,
    user_decision_to_dict,
)
from agent_sec_cli.skill_ledger.core.manifest_integrity import (
    verify_manifest_integrity,
)
from agent_sec_cli.skill_ledger.core.resolver import (
    resolve_activation,
)
from agent_sec_cli.skill_ledger.core.version_chain import (
    SKILL_META_DIR,
    ensure_skill_meta,
    list_version_ids,
    load_latest_manifest,
    load_version_manifest,
    save_manifest,
    snapshot_dir_path,
)
from agent_sec_cli.skill_ledger.errors import SkillLedgerError
from agent_sec_cli.skill_ledger.models.manifest import (
    SignedManifest,
    UserDecision,
)
from agent_sec_cli.skill_ledger.signing.base import SigningBackend
from agent_sec_cli.skill_ledger.utils import utc_now_iso, validate_skill_dir

_TRUSTED_CURRENT_STATUSES = {"pass", "warn", "deny"}
_ALLOWING_DECISIONS = {"allow", "always_allow", "rollback"}
_ROOT_COPY_EXCLUDED = {SKILL_META_DIR, ".git"}
_DECISION_LOCK = "decision.lock"


def decide_skill(
    skill_dir: str,
    backend: SigningBackend,
    *,
    action: str,
    target_version_id: str | None = None,
    reason: str | None = None,
) -> dict[str, Any]:
    """Apply a user decision to a skill and refresh activation."""
    validate_skill_dir(skill_dir)
    if action == "rollback":
        if not target_version_id:
            target_version_id = _default_rollback_target(skill_dir, backend)
        return rollback_skill(
            skill_dir,
            backend,
            target_version_id=target_version_id,
            reason=reason,
        )
    if action not in {"allow", "always_allow", "block"}:
        raise SkillLedgerError(
            "decision action must be one of: allow, always_allow, block, rollback"
        )

    with _skill_decision_lock(skill_dir):
        manifest, status_result = _ensure_current_signed_version(skill_dir, backend)
        manifest.userDecision = UserDecision(action=action, reason=reason)
        _sign_and_save(skill_dir, manifest, backend)
        activation = _refresh_activation(skill_dir, backend)
    return _decision_payload(
        skill_dir,
        manifest,
        status=status_result.get("status"),
        activation=activation,
    )


def clear_decision(skill_dir: str, backend: SigningBackend) -> dict[str, Any]:
    """Remove the latest version's user decision and refresh activation."""
    validate_skill_dir(skill_dir)
    with _skill_decision_lock(skill_dir):
        manifest = load_latest_manifest(skill_dir)
        if manifest is None:
            raise SkillLedgerError(
                "cannot clear decision: skill has no signed manifest"
            )
        valid, error = verify_manifest_integrity(manifest, backend)
        if not valid:
            raise SkillLedgerError(
                f"cannot clear decision on untrusted manifest: {error}"
            )
        manifest.userDecision = None
        _sign_and_save(skill_dir, manifest, backend)
        activation = _refresh_activation(skill_dir, backend)
    return _decision_payload(skill_dir, manifest, status=None, activation=activation)


def rollback_skill(
    skill_dir: str,
    backend: SigningBackend,
    *,
    target_version_id: str,
    reason: str | None = None,
) -> dict[str, Any]:
    """Restore a trusted snapshot to the root and record rollback as a new version."""
    validate_skill_dir(skill_dir)
    with _skill_decision_lock(skill_dir):
        target_manifest = _load_trusted_version(skill_dir, target_version_id, backend)
        target_snapshot = snapshot_dir_path(skill_dir, target_version_id)
        if not target_snapshot.is_dir():
            raise SkillLedgerError(f"rollback snapshot not found: {target_version_id}")
        if not snapshot_matches_manifest(target_snapshot, target_manifest):
            raise SkillLedgerError(
                f"rollback snapshot does not match manifest: {target_version_id}"
            )

        backup_dir = _backup_root(skill_dir)
        try:
            _replace_root_from_snapshot(skill_dir, target_snapshot)
            scan_skill(skill_dir, backend, force=True)
            manifest = load_latest_manifest(skill_dir)
            if manifest is None:
                raise SkillLedgerError("rollback scan did not create a manifest")
            manifest.userDecision = UserDecision(
                action="rollback",
                targetVersionId=target_version_id,
                reason=reason,
            )
            _sign_and_save(skill_dir, manifest, backend)
        except BaseException:
            _replace_root_from_snapshot(skill_dir, backup_dir)
            raise

        activation = _refresh_activation(skill_dir, backend)
        return _decision_payload(
            skill_dir,
            manifest,
            status=manifest.scanStatus,
            activation=activation,
            extra={"rollbackBackup": str(backup_dir)},
        )


def show_skill(
    skill_dir: str,
    backend: SigningBackend,
    *,
    policy: str | None = None,
) -> dict[str, Any]:
    """Return latest, active, decision, and consistency information."""
    validate_skill_dir(skill_dir)
    resolved_policy = resolve_activation_policy(
        {"activationPolicy": policy} if policy is not None else None
    )
    status_result = check(skill_dir, backend)
    summary = build_exposure_summary(
        skill_dir,
        backend,
        status_result=status_result,
    )
    latest_manifest = safe_load_latest_manifest(skill_dir)
    active_version = summary.get("activeVersionId")
    active_manifest = (
        load_version_manifest(skill_dir, active_version) if active_version else None
    )
    root_matches_active = _root_matches_manifest(skill_dir, active_manifest)
    consistency_reason = _show_consistency_reason(
        summary=summary,
        latest_manifest=latest_manifest,
        active_manifest=active_manifest,
        active_version=active_version,
        root_matches_active=root_matches_active,
        policy=resolved_policy,
    )
    warnings = [summary["message"]] if summary["message"] is not None else []
    return {
        **summary,
        "skillName": Path(skill_dir).name,
        "activationPolicy": resolved_policy,
        "latest": _manifest_summary(latest_manifest, status_result),
        "active": _manifest_summary(active_manifest, None),
        "rootMatchesActive": root_matches_active,
        "consistencyReason": consistency_reason,
        "findings": status_result.get("findings", []),
        "warnings": warnings,
    }


def export_skill(
    skill_dir: str,
    backend: SigningBackend,
    *,
    version: str,
    output: str,
    policy: str | None = None,
) -> dict[str, Any]:
    """Export a signed snapshot plus manifest and findings for user review."""
    validate_skill_dir(skill_dir)
    version_id = _resolve_export_version(skill_dir, backend, version, policy=policy)
    manifest = _load_trusted_version(skill_dir, version_id, backend)
    snapshot = snapshot_dir_path(skill_dir, version_id)
    if not snapshot.is_dir():
        raise SkillLedgerError(f"snapshot not found for version {version_id}")
    if not snapshot_matches_manifest(snapshot, manifest):
        raise SkillLedgerError(f"export snapshot does not match manifest: {version_id}")
    out_dir = Path(output)
    if out_dir.exists() and any(out_dir.iterdir()):
        raise SkillLedgerError(
            f"export output already exists and is not empty: {out_dir}"
        )
    out_dir.mkdir(parents=True, exist_ok=True)
    snapshot_out = out_dir / "snapshot"
    if snapshot_out.exists():
        shutil.rmtree(snapshot_out)
    shutil.copytree(snapshot, snapshot_out)
    (out_dir / "manifest.json").write_text(manifest.to_json() + "\n", encoding="utf-8")
    findings = _collect_findings(manifest)
    (out_dir / "findings.json").write_text(
        json.dumps(findings, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return {
        "skillName": Path(skill_dir).name,
        "versionId": version_id,
        "output": str(out_dir),
        "snapshot": str(snapshot_out),
        "manifest": str(out_dir / "manifest.json"),
        "findings": str(out_dir / "findings.json"),
    }


def _ensure_current_signed_version(
    skill_dir: str,
    backend: SigningBackend,
) -> tuple[SignedManifest, dict[str, Any]]:
    status_result = check(skill_dir, backend)
    if status_result.get("status") not in _TRUSTED_CURRENT_STATUSES:
        scan_skill(skill_dir, backend, force=True)
        status_result = check(skill_dir, backend)
    if status_result.get("status") not in _TRUSTED_CURRENT_STATUSES:
        raise SkillLedgerError(
            f"cannot decide on untrusted current skill state: {status_result.get('status')}"
        )
    manifest = load_latest_manifest(skill_dir)
    if manifest is None:
        raise SkillLedgerError("scan did not create a signed manifest")
    return manifest, status_result


def _default_rollback_target(skill_dir: str, backend: SigningBackend) -> str:
    activation = resolve_activation(
        skill_dir,
        backend,
        policy=resolve_activation_policy(),
        write_activation=False,
    )
    active_version = activation.get("activeVersionId")
    if not active_version:
        raise SkillLedgerError(
            "cannot choose rollback target: no active version under current "
            "user decision or activationPolicy"
        )
    return str(active_version)


def _load_trusted_version(
    skill_dir: str,
    version_id: str,
    backend: SigningBackend,
) -> SignedManifest:
    manifest = load_version_manifest(skill_dir, version_id)
    if manifest is None:
        raise SkillLedgerError(f"unknown skill version: {version_id}")
    if manifest.versionId != version_id:
        raise SkillLedgerError(f"version manifest id mismatch: {version_id}")
    valid, error = verify_manifest_integrity(manifest, backend)
    if not valid:
        raise SkillLedgerError(f"untrusted version {version_id}: {error}")
    return manifest


def _sign_and_save(
    skill_dir: str,
    manifest: SignedManifest,
    backend: SigningBackend,
) -> None:
    manifest.updatedAt = utc_now_iso()
    _sign_manifest(manifest, backend)
    save_manifest(skill_dir, manifest, write_version=True)


def _refresh_activation(skill_dir: str, backend: SigningBackend) -> dict[str, Any]:
    return resolve_activation(
        skill_dir,
        backend,
        policy=resolve_activation_policy(),
        write_activation=True,
    )


def _decision_payload(
    skill_dir: str,
    manifest: SignedManifest,
    *,
    status: str | None,
    activation: dict[str, Any],
    extra: dict[str, Any] | None = None,
) -> dict[str, Any]:
    data: dict[str, Any] = {
        "status": "decided",
        "skillName": Path(skill_dir).name,
        "versionId": manifest.versionId,
        "scanStatus": manifest.scanStatus,
        "manifestHash": manifest.manifestHash,
        "userDecision": _decision_dict(manifest),
        "activation": activation,
    }
    if status is not None:
        data["currentStatus"] = status
    if extra:
        data.update(extra)
    return data


def _decision_dict(manifest: SignedManifest | None) -> dict[str, Any] | None:
    if manifest is None or manifest.userDecision is None:
        return None
    return user_decision_to_dict(manifest.userDecision)


def _manifest_summary(
    manifest: SignedManifest | None,
    status_result: dict[str, Any] | None,
) -> dict[str, Any] | None:
    if manifest is None:
        return None
    status = (
        status_result.get("status")
        if status_result is not None
        else manifest.scanStatus
    )
    return {
        "versionId": manifest.versionId,
        "status": status,
        "scanStatus": manifest.scanStatus,
        "manifestHash": manifest.manifestHash,
        "userDecision": _decision_dict(manifest),
    }


def _show_consistency_reason(
    *,
    summary: dict[str, Any],
    latest_manifest: SignedManifest | None,
    active_manifest: SignedManifest | None,
    active_version: str | None,
    root_matches_active: bool | None,
    policy: str,
) -> str | None:
    reason_code = summary.get("reasonCode")
    latest_version = latest_manifest.versionId if latest_manifest is not None else None
    if reason_code == "user_block":
        return "user decision block hides this skill"
    if reason_code in {"root_drift", "tampered"}:
        return summary.get("message")
    if reason_code in {
        "latest_risk_fallback_to_previous",
        "latest_risk_hidden",
        "latest_risk_pending_decision",
    }:
        return summary.get("message")
    if latest_version != active_version:
        active_decision = active_manifest.userDecision if active_manifest else None
        if (
            active_decision is not None
            and active_decision.action in _ALLOWING_DECISIONS
        ):
            return (
                f"user decision {active_decision.action} pins active version "
                f"{active_version or 'none'} instead of latest {latest_version or 'none'}"
            )
        if latest_manifest is None:
            return "no signed manifest snapshot is available"
        if active_version is None:
            return (
                f"activationPolicy {policy} hides latest version {latest_version} "
                f"with scanStatus {latest_manifest.scanStatus}"
            )
        return (
            f"activationPolicy {policy} exposes version {active_version} "
            f"instead of latest {latest_version or 'none'}"
        )
    if root_matches_active is False:
        return (
            "root drift: current skill root does not match active snapshot "
            f"{active_version or 'none'}"
        )
    return None


def _newest_trusted_decision(
    skill_dir: str,
    backend: SigningBackend,
) -> tuple[str, UserDecision] | None:
    saw_newer_trusted_version = False
    for version_id in reversed(list_version_ids(skill_dir)):
        try:
            manifest = load_version_manifest(skill_dir, version_id)
        except (json.JSONDecodeError, ValueError):
            continue
        if manifest is None:
            continue
        if manifest.versionId != version_id:
            continue
        valid, _ = verify_manifest_integrity(manifest, backend)
        if not valid:
            continue
        if not snapshot_matches_manifest(
            snapshot_dir_path(skill_dir, version_id),
            manifest,
        ):
            continue
        decision = manifest.userDecision
        if decision is None:
            saw_newer_trusted_version = True
            continue
        if decision.action == "block" and saw_newer_trusted_version:
            return None
        if decision.action == "block" or decision.action in _ALLOWING_DECISIONS:
            return version_id, decision
        saw_newer_trusted_version = True
    return None


def _root_matches_manifest(
    skill_dir: str,
    manifest: SignedManifest | None,
) -> bool | None:
    if manifest is None:
        return None
    try:
        root_hashes = compute_file_hashes(skill_dir)
    except ValueError:
        return False
    return bool(diff_file_hashes(manifest.fileHashes, root_hashes)["match"])


def _resolve_export_version(
    skill_dir: str,
    backend: SigningBackend,
    version: str,
    *,
    policy: str | None,
) -> str:
    if version == "latest":
        manifest = load_latest_manifest(skill_dir)
        if manifest is None:
            raise SkillLedgerError("skill has no latest version to export")
        return manifest.versionId
    if version == "active":
        activation = resolve_activation(
            skill_dir,
            backend,
            policy=policy or resolve_activation_policy(),
            write_activation=False,
        )
        active_version = activation.get("activeVersionId")
        if not active_version:
            raise SkillLedgerError("skill has no active version to export")
        return str(active_version)
    if version in list_version_ids(skill_dir):
        return version
    raise SkillLedgerError(f"unknown export version: {version}")


def _collect_findings(manifest: SignedManifest) -> list[dict[str, Any]]:
    return [finding for scan in manifest.scans for finding in scan.findings]


@contextmanager
def _skill_decision_lock(skill_dir: str):
    meta = ensure_skill_meta(skill_dir)
    lock_path = meta / _DECISION_LOCK
    with lock_path.open("a", encoding="utf-8") as lock_file:
        fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(lock_file.fileno(), fcntl.LOCK_UN)


def _backup_root(skill_dir: str) -> Path:
    meta = ensure_skill_meta(skill_dir)
    backup = (
        meta
        / "backups"
        / f"rollback-{utc_now_iso().replace(':', '').replace('+', 'Z')}"
    )
    _copy_root(Path(skill_dir), backup)
    return backup


def _replace_root_from_snapshot(skill_dir: str, snapshot: Path) -> None:
    root = Path(skill_dir)
    for entry in list(root.iterdir()):
        if entry.name in _ROOT_COPY_EXCLUDED:
            continue
        if entry.is_symlink():
            entry.unlink()
        elif entry.is_dir():
            shutil.rmtree(entry)
        elif entry.is_file():
            entry.unlink()
        else:
            entry.unlink()
    _copy_root(snapshot, root)


def _copy_root(src: Path, dst: Path) -> None:
    dst.mkdir(parents=True, exist_ok=True)
    for entry in sorted(src.rglob("*")):
        if entry.is_symlink():
            continue
        rel = entry.relative_to(src)
        if any(part in _ROOT_COPY_EXCLUDED for part in rel.parts):
            continue
        target = dst / rel
        if entry.is_dir():
            target.mkdir(parents=True, exist_ok=True)
        elif entry.is_file():
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(entry, target)
