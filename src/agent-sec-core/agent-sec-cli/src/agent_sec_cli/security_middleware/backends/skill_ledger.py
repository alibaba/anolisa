"""Skill-ledger backend — dispatch to skill-ledger core operations.

Routes the ``command`` kwarg to the appropriate handler, returning a
unified :class:`ActionResult`.
"""

import json
from typing import Any

from agent_sec_cli.security_middleware.backends.base import BaseBackend
from agent_sec_cli.security_middleware.context import RequestContext
from agent_sec_cli.security_middleware.result import ActionResult
from agent_sec_cli.skill_ledger.config import resolve_skill_dirs
from agent_sec_cli.skill_ledger.core.auditor import audit
from agent_sec_cli.skill_ledger.core.certifier import certify, certify_batch
from agent_sec_cli.skill_ledger.core.checker import check, check_batch
from agent_sec_cli.skill_ledger.core.status import ledger_status
from agent_sec_cli.skill_ledger.scanner.registry import ScannerRegistry
from agent_sec_cli.skill_ledger.signing.ed25519 import NativeEd25519Backend
from agent_sec_cli.skill_ledger.signing.key_manager import (
    archive_current_public_key,
    ensure_keys_not_exist,
)


class SkillLedgerBackend(BaseBackend):
    """Dispatch backend for all skill-ledger subcommands."""

    def execute(self, ctx: RequestContext, **kwargs: Any) -> ActionResult:
        """Dispatch to the handler identified by ``command``."""
        command = kwargs.pop("command", "")
        handler_name = f"_do_{command.replace('-', '_')}"
        handler = getattr(self, handler_name, None)
        if handler is None:
            return ActionResult(
                success=False,
                error=f"Unknown skill-ledger command: {command!r}",
                exit_code=1,
            )
        return handler(ctx, **kwargs)

    # ------------------------------------------------------------------
    # Handlers
    # ------------------------------------------------------------------

    def _do_init_keys(
        self,
        ctx: RequestContext,
        *,
        force: bool = False,
        passphrase: str | None = None,
        **kw: Any,
    ) -> ActionResult:
        try:
            ensure_keys_not_exist(force=force)
        except Exception as exc:
            return ActionResult(success=False, error=str(exc), exit_code=1)

        # Archive the old public key into the keyring so that existing
        # signatures remain verifiable after key rotation.
        if force:
            try:
                archive_current_public_key()
            except OSError as exc:
                return ActionResult(
                    success=False,
                    error=f"Failed to archive public key before rotation: {exc}",
                    exit_code=1,
                )

        backend = NativeEd25519Backend()
        try:
            result = backend.generate_keys(passphrase)
        except Exception as exc:
            return ActionResult(success=False, error=str(exc), exit_code=1)

        data = {"command": "init-keys", **result}
        return ActionResult(
            success=True,
            stdout=json.dumps(data, ensure_ascii=False) + "\n",
            data=data,
        )

    def _do_check(
        self,
        ctx: RequestContext,
        *,
        skill_dir: str | None = None,
        all_skills: bool = False,
        **kw: Any,
    ) -> ActionResult:
        backend = NativeEd25519Backend()

        try:
            if all_skills:
                dirs = resolve_skill_dirs()
                if not dirs:
                    return ActionResult(
                        success=False,
                        error="No skill directories found in config.json",
                        exit_code=1,
                    )
                results = check_batch(dirs, backend)
                has_critical = any(
                    r.get("status") in ("tampered", "deny", "error") for r in results
                )
                data = {"command": "check", "results": results}
                return ActionResult(
                    success=not has_critical,
                    stdout=json.dumps({"results": results}, ensure_ascii=False) + "\n",
                    data=data,
                    exit_code=1 if has_critical else 0,
                )
            else:
                if skill_dir is None:
                    return ActionResult(
                        success=False,
                        error="skill_dir is required (or use --all)",
                        exit_code=1,
                    )
                result = check(skill_dir, backend)
                status = result.get("status", "")
                is_critical = status in ("tampered", "deny")
                return ActionResult(
                    success=not is_critical,
                    stdout=json.dumps(result, ensure_ascii=False) + "\n",
                    data={"command": "check", **result},
                    exit_code=1 if is_critical else 0,
                )
        except Exception as exc:
            error_data = {"status": "error", "error": str(exc)}
            return ActionResult(
                success=False,
                stdout=json.dumps(error_data, ensure_ascii=False) + "\n",
                data={"command": "check", **error_data},
                exit_code=1,
            )

    def _do_certify(
        self,
        ctx: RequestContext,
        *,
        skill_dir: str | None = None,
        all_skills: bool = False,
        findings: str | None = None,
        scanner: str = "skill-vetter",
        scanner_version: str | None = None,
        scanner_names: list[str] | None = None,
        **kw: Any,
    ) -> ActionResult:
        backend = NativeEd25519Backend()

        try:
            if all_skills:
                if findings:
                    return ActionResult(
                        success=False,
                        error="--all and --findings are incompatible",
                        exit_code=1,
                    )
                dirs = resolve_skill_dirs()
                if not dirs:
                    return ActionResult(
                        success=False,
                        error="No skill directories found in config.json",
                        exit_code=1,
                    )
                results = certify_batch(
                    dirs,
                    backend,
                    findings_path=findings,
                    scanner=scanner,
                    scanner_version=scanner_version,
                    scanner_names=scanner_names,
                )
                has_error = any(r.get("status") == "error" for r in results)
                data = {"command": "certify", "results": results}
                return ActionResult(
                    success=not has_error,
                    stdout=json.dumps({"results": results}, ensure_ascii=False) + "\n",
                    data=data,
                    exit_code=1 if has_error else 0,
                )
            else:
                if skill_dir is None:
                    return ActionResult(
                        success=False,
                        error="skill_dir is required (or use --all)",
                        exit_code=1,
                    )
                result = certify(
                    skill_dir,
                    backend,
                    findings_path=findings,
                    scanner=scanner,
                    scanner_version=scanner_version,
                    scanner_names=scanner_names,
                )
                return ActionResult(
                    success=True,
                    stdout=json.dumps(result, ensure_ascii=False) + "\n",
                    data={"command": "certify", **result},
                )
        except Exception as exc:
            return ActionResult(success=False, error=str(exc), exit_code=1)

    def _do_status(
        self,
        ctx: RequestContext,
        *,
        verbose: bool = False,
        **kw: Any,
    ) -> ActionResult:
        backend = NativeEd25519Backend()

        try:
            result = ledger_status(backend, verbose=verbose)
            data = {"command": "status", **result}
            return ActionResult(
                success=True,
                stdout=json.dumps(data, ensure_ascii=False) + "\n",
                data=data,
            )
        except Exception as exc:
            return ActionResult(success=False, error=str(exc), exit_code=1)

    def _do_audit(
        self,
        ctx: RequestContext,
        *,
        skill_dir: str,
        verify_snapshots: bool = False,
        **kw: Any,
    ) -> ActionResult:
        backend = NativeEd25519Backend()

        try:
            result = audit(skill_dir, backend, verify_snapshots=verify_snapshots)
        except Exception as exc:
            return ActionResult(success=False, error=str(exc), exit_code=1)

        return ActionResult(
            success=result["valid"],
            stdout=json.dumps(result, ensure_ascii=False) + "\n",
            data={"command": "audit", **result},
            exit_code=0 if result["valid"] else 1,
        )

    def _do_list_scanners(self, ctx: RequestContext, **kw: Any) -> ActionResult:
        registry = ScannerRegistry.from_config()
        scanners = registry.list_scanners(enabled_only=False)

        scanner_data = []
        for s in scanners:
            scanner_data.append(
                {
                    "name": s.name,
                    "type": s.type,
                    "parser": s.parser,
                    "enabled": s.enabled,
                    "description": s.description,
                }
            )

        data = {"command": "list-scanners", "scanners": scanner_data}
        return ActionResult(
            success=True,
            stdout=json.dumps(data, ensure_ascii=False) + "\n",
            data=data,
        )
