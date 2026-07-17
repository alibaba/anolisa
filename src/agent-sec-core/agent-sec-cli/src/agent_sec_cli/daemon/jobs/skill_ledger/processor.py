"""Synchronous Skill Ledger scan and activation processing."""

from typing import Any

from agent_sec_cli.daemon.jobs.skill_ledger.protocol import SkillFsChange
from agent_sec_cli.skill_ledger.errors import UnresolvedLiveRootError


def process_skill_change(change: SkillFsChange) -> dict[str, Any]:
    """Run scan and activation resolution for a debounced SkillFS change."""
    backend = _ensure_default_backend()
    policy = _resolve_activation_policy()
    scan_result: dict[str, Any] | None = None
    scan_error: str | None = None
    scan_exception: Exception | None = None
    try:
        scan_result = _scan_skill(str(change.skill_dir), backend)
    except Exception as exc:
        if _is_unresolved_live_root_error(exc):
            return _skipped_unmanaged_result(change, str(exc))
        scan_error = str(exc)
        scan_exception = exc

    try:
        activation_result = _resolve_activation(str(change.skill_dir), backend, policy)
    except Exception as exc:
        if _is_unresolved_live_root_error(exc):
            if scan_exception is None:
                return _skipped_unmanaged_result(change, str(exc))
            # A prior scanner failure is the health signal; keep it attached
            # instead of downgrading the later live-root failure to skipped.
            raise exc from scan_exception
        if scan_exception is not None:
            raise exc from scan_exception
        raise
    result: dict[str, Any] = {
        "status": "processed" if scan_error is None else "error",
        "skill": change.to_dict(),
        "scan": scan_result,
        "activation": activation_result,
    }
    if scan_error is not None:
        result["error"] = scan_error
    return result


def _is_unresolved_live_root_error(exc: Exception) -> bool:
    return isinstance(exc, UnresolvedLiveRootError)


def _skipped_unmanaged_result(change: SkillFsChange, message: str) -> dict[str, Any]:
    return {
        "status": "skipped",
        "reasonCode": "unmanaged_skill_root",
        "message": message,
        "skill": change.to_dict(),
        "scan": None,
        "activation": None,
    }


def _ensure_default_backend() -> Any:
    from agent_sec_cli.skill_ledger.signing.ed25519 import (  # noqa: PLC0415
        NativeEd25519Backend,
    )
    from agent_sec_cli.skill_ledger.signing.key_manager import (  # noqa: PLC0415
        keys_exist,
    )

    if not keys_exist():
        NativeEd25519Backend().generate_keys(passphrase=None)
    return NativeEd25519Backend()


def _scan_skill(skill_dir: str, backend: Any) -> dict[str, Any]:
    from agent_sec_cli.skill_ledger.core.certifier import (  # noqa: PLC0415
        scan_skill,
    )

    return scan_skill(skill_dir, backend, force=False)


def _resolve_activation(skill_dir: str, backend: Any, policy: str) -> dict[str, Any]:
    from agent_sec_cli.skill_ledger.core.resolver import (  # noqa: PLC0415
        resolve_activation,
    )

    return resolve_activation(skill_dir, backend, policy=policy)


def _resolve_activation_policy() -> str:
    from agent_sec_cli.skill_ledger.config import (  # noqa: PLC0415
        resolve_activation_policy,
    )

    return resolve_activation_policy()
