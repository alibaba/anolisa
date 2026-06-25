"""Unified Skill Ledger exposure and warning summary."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from agent_sec_cli.skill_ledger.activation_policy import (
    allowed_scan_statuses_for_policy,
)
from agent_sec_cli.skill_ledger.core.checker import check
from agent_sec_cli.skill_ledger.core.file_hasher import (
    compute_snapshot_file_hashes,
    diff_file_hashes,
)
from agent_sec_cli.skill_ledger.core.manifest_integrity import (
    verify_manifest_integrity,
)
from agent_sec_cli.skill_ledger.core.version_chain import (
    SKILL_META_DIR,
    VERSIONS_DIR,
    list_version_ids,
    load_latest_manifest,
    load_version_manifest,
    snapshot_dir_path,
)
from agent_sec_cli.skill_ledger.models.manifest import (
    SignedManifest,
    UserDecision,
)
from agent_sec_cli.skill_ledger.signing.base import SigningBackend
from agent_sec_cli.skill_ledger.utils import validate_skill_dir

_ALLOWING_USER_DECISIONS = {"allow", "always_allow", "rollback"}
_USER_DECISION_STATUSES = frozenset({"pass", "warn", "deny", "none"})
_EXPOSABLE_STATUSES = allowed_scan_statuses_for_policy("pass_warn_only")
_BLOCKED_BY_USER = object()
PENDING_DECISION_SNAPSHOT = "__pending_decision__.snapshot"
PENDING_DECISION_TARGET = f"{SKILL_META_DIR}/{VERSIONS_DIR}/{PENDING_DECISION_SNAPSHOT}"


def build_exposure_summary(skill_dir: str, backend: SigningBackend) -> dict[str, Any]:
    """Return the single source of truth for runtime exposure and warnings.

    The summary intentionally stays small so hooks, ``show``, and activation
    resolution can consume the same decision without learning separate policy
    branches.
    """
    validate_skill_dir(skill_dir)
    status_result = check(skill_dir, backend)
    latest_status = str(status_result.get("status", "unknown"))
    latest_manifest = _safe_load_latest_manifest(skill_dir)
    latest_version = _latest_version_id(status_result, latest_manifest)

    decision_candidate = _find_user_decision_candidate(skill_dir, backend)
    if decision_candidate is _BLOCKED_BY_USER:
        return _summary(
            latest_status=latest_status,
            latest_version=latest_version,
            active_version=None,
            user_decision=_newest_block_decision(skill_dir, backend),
            reason_code="user_block",
            message=None,
        )
    if isinstance(decision_candidate, tuple):
        version_id, decision = decision_candidate
        return _summary(
            latest_status=latest_status,
            latest_version=latest_version,
            active_version=version_id,
            user_decision=decision,
            reason_code=f"user_{decision.action}",
            message=None,
        )

    candidate = _find_policy_candidate(skill_dir, backend)
    active_version = candidate[0] if candidate is not None else None

    if latest_status in {"pass", "warn"}:
        if active_version == latest_version and active_version is not None:
            return _summary(
                latest_status=latest_status,
                latest_version=latest_version,
                active_version=active_version,
                user_decision=None,
                reason_code="normal",
                message=None,
            )
        return _summary(
            latest_status=latest_status,
            latest_version=latest_version,
            active_version=active_version,
            user_decision=None,
            reason_code="tampered",
            message=_activation_message(
                f"Latest {latest_status} snapshot is not a trusted activation target",
                active_version,
            ),
        )

    if latest_status == "drifted":
        return _summary(
            latest_status=latest_status,
            latest_version=latest_version,
            active_version=active_version,
            user_decision=None,
            reason_code="root_drift",
            message=_activation_message(
                f"Current skill root drifted from latest signed version {latest_version or 'none'}",
                active_version,
            ),
        )

    if latest_status == "tampered":
        return _summary(
            latest_status=latest_status,
            latest_version=latest_version,
            active_version=active_version,
            user_decision=None,
            reason_code="tampered",
            message=_activation_message(
                f"Latest skill metadata is tampered for version {latest_version or 'none'}",
                active_version,
            ),
        )

    reason_code = (
        "latest_risk_fallback_to_previous"
        if active_version is not None
        else "latest_risk_pending_decision"
    )
    return _summary(
        latest_status=latest_status,
        latest_version=latest_version,
        active_version=active_version,
        user_decision=None,
        reason_code=reason_code,
        message=_activation_message(
            f"Latest skill status is {latest_status} for version {latest_version or 'none'}",
            active_version,
        ),
    )


def exposure_target(version_id: str) -> str:
    """Return the SkillFS activation target for a snapshot version."""
    return f"{SKILL_META_DIR}/{VERSIONS_DIR}/{version_id}.snapshot"


def pending_decision_target() -> str:
    """Return the SkillFS activation target for the safe pending review stub."""
    return PENDING_DECISION_TARGET


def is_pending_decision_target(target: Any) -> bool:
    """Return whether *target* is the safe pending review stub target."""
    return target == PENDING_DECISION_TARGET


def _summary(
    *,
    latest_status: str,
    latest_version: str | None,
    active_version: str | None,
    user_decision: UserDecision | None,
    reason_code: str,
    message: str | None,
) -> dict[str, Any]:
    target = exposure_target(active_version) if active_version else None
    if target is None and user_decision is None and message is not None:
        target = pending_decision_target()
    return {
        "latestStatus": latest_status,
        "latestVersionId": latest_version,
        "activeVersionId": active_version,
        "target": target,
        "userDecision": _decision_dict(user_decision),
        "reasonCode": reason_code,
        "message": message,
    }


def _activation_message(prefix: str, active_version: str | None) -> str:
    if active_version is None:
        return (
            f"{prefix}; active skill is a safe review stub pending user decision. "
            "Review with 'agent-sec-cli skill-ledger show' or "
            "'agent-sec-cli skill-ledger export', then choose with "
            "'agent-sec-cli skill-ledger decide'."
        )
    return f"{prefix}; active version is {active_version}."


def _latest_version_id(
    status_result: dict[str, Any],
    latest_manifest: SignedManifest | None,
) -> str | None:
    value = status_result.get("versionId")
    if isinstance(value, str) and value:
        return value
    if latest_manifest is not None:
        return latest_manifest.versionId
    return None


def _safe_load_latest_manifest(skill_dir: str) -> SignedManifest | None:
    try:
        return load_latest_manifest(skill_dir)
    except (json.JSONDecodeError, ValueError):
        return None


def _find_user_decision_candidate(
    skill_dir: str | Path,
    backend: SigningBackend,
) -> tuple[str, UserDecision] | object | None:
    saw_newer_trusted_version = False
    for version_id in reversed(list_version_ids(skill_dir)):
        manifest = _load_trusted_activation_manifest(
            skill_dir,
            version_id,
            backend,
            allowed_statuses=_USER_DECISION_STATUSES,
        )
        if manifest is None:
            continue
        decision = manifest.userDecision
        if decision is None:
            saw_newer_trusted_version = True
            continue
        if decision.action == "block":
            return None if saw_newer_trusted_version else _BLOCKED_BY_USER
        if decision.action in _ALLOWING_USER_DECISIONS:
            return version_id, decision
        saw_newer_trusted_version = True
    return None


def _newest_block_decision(
    skill_dir: str | Path,
    backend: SigningBackend,
) -> UserDecision | None:
    for version_id in reversed(list_version_ids(skill_dir)):
        manifest = _load_trusted_activation_manifest(
            skill_dir,
            version_id,
            backend,
            allowed_statuses=_USER_DECISION_STATUSES,
        )
        if manifest is None:
            continue
        if (
            manifest.userDecision is not None
            and manifest.userDecision.action == "block"
        ):
            return manifest.userDecision
    return None


def _find_policy_candidate(
    skill_dir: str | Path,
    backend: SigningBackend,
) -> tuple[str, str] | None:
    for version_id in reversed(list_version_ids(skill_dir)):
        manifest = _load_trusted_activation_manifest(
            skill_dir,
            version_id,
            backend,
            allowed_statuses=_EXPOSABLE_STATUSES,
        )
        if manifest is None:
            continue
        return version_id, exposure_target(version_id)
    return None


def _load_trusted_activation_manifest(
    skill_dir: str | Path,
    version_id: str,
    backend: SigningBackend,
    *,
    allowed_statuses: frozenset[str],
) -> SignedManifest | None:
    try:
        manifest = load_version_manifest(skill_dir, version_id)
    except (json.JSONDecodeError, ValueError):
        return None
    if manifest is None:
        return None
    if manifest.versionId != version_id:
        return None
    if manifest.scanStatus not in allowed_statuses:
        return None
    valid, _ = verify_manifest_integrity(manifest, backend)
    if not valid:
        return None
    if not _snapshot_matches_manifest(skill_dir, version_id, manifest):
        return None
    return manifest


def _snapshot_matches_manifest(
    skill_dir: str | Path,
    version_id: str,
    manifest: SignedManifest,
) -> bool:
    snapshot = snapshot_dir_path(skill_dir, version_id)
    try:
        snapshot_hashes = compute_snapshot_file_hashes(snapshot)
    except ValueError:
        return False
    return bool(diff_file_hashes(manifest.fileHashes, snapshot_hashes)["match"])


def _decision_dict(decision: UserDecision | None) -> dict[str, Any] | None:
    if decision is None:
        return None
    return decision.model_dump(exclude_none=True)
