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
from agent_sec_cli.skill_ledger.core.certifier import certify, scan_batch, scan_skill
from agent_sec_cli.skill_ledger.core.checker import check, check_batch
from agent_sec_cli.skill_ledger.core.status import ledger_status
from agent_sec_cli.skill_ledger.scanner.registry import ScannerRegistry
from agent_sec_cli.skill_ledger.signing.ed25519 import NativeEd25519Backend
from agent_sec_cli.skill_ledger.signing.key_manager import (
    archive_current_public_key,
    ensure_keys_not_exist,
    keys_exist,
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

    def _generate_keys(self, *, force: bool = False, passphrase: str | None = None) -> dict:
        """Generate key material and return the backend result dict."""
        ensure_keys_not_exist(force=force)
        # Archive the old public key into the keyring so that existing
        # signatures remain verifiable after key rotation.
        if force:
            archive_current_public_key()
        backend = NativeEd25519Backend()
        return backend.generate_keys(passphrase)

    def _ensure_keys(self) -> tuple[bool, dict[str, Any] | None]:
        """Create default unencrypted keys when absent."""
        if keys_exist():
            return False, None
        return True, self._generate_keys(force=False, passphrase=None)

    def _do_init(
        self,
        ctx: RequestContext,
        *,
        baseline: bool = True,
        passphrase: str | None = None,
        force_keys: bool = False,
        scanner_names: list[str] | None = None,
        **kw: Any,
    ) -> ActionResult:
        key_created = False
        key_result: dict[str, Any] | None = None
        try:
            if force_keys or not keys_exist():
                key_result = self._generate_keys(force=force_keys, passphrase=passphrase)
                key_created = True

            results: list[dict[str, Any]] = []
            if baseline:
                dirs = resolve_skill_dirs()
                if dirs:
                    backend = NativeEd25519Backend()
                    results = scan_batch(dirs, backend, scanner_names=scanner_names)
            has_error = any(r.get("status") == "error" for r in results)
            data = {
                "command": "init",
                "keyCreated": key_created,
                "key": key_result,
                "baseline": baseline,
                "results": results,
            }
            return ActionResult(
                success=not has_error,
                stdout=json.dumps(data, ensure_ascii=False) + "\n",
                data=data,
                exit_code=1 if has_error else 0,
            )
        except Exception as exc:
            return ActionResult(success=False, error=str(exc), exit_code=1)

    def _do_init_keys(
        self,
        ctx: RequestContext,
        *,
        force: bool = False,
        passphrase: str | None = None,
        **kw: Any,
    ) -> ActionResult:
        try:
            result = self._generate_keys(force=force, passphrase=passphrase)
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
        delete_findings: bool = False,
        **kw: Any,
    ) -> ActionResult:
        try:
            if all_skills or scanner_names:
                return ActionResult(
                    success=False,
                    error="certify imports external findings only; use 'skill-ledger scan' for built-in scanners",
                    exit_code=1,
                )
            if skill_dir is None:
                return ActionResult(
                    success=False,
                    error="skill_dir is required",
                    exit_code=1,
                )
            if findings is None:
                return ActionResult(
                    success=False,
                    error="--findings is required for certify; use 'skill-ledger scan' for built-in scanners",
                    exit_code=1,
                )
            key_created, key_result = self._ensure_keys()
            backend = NativeEd25519Backend()
            result = certify(
                skill_dir,
                backend,
                findings_path=findings,
                scanner=scanner,
                scanner_version=scanner_version,
                delete_findings=delete_findings,
            )
            result["keyCreated"] = key_created
            if key_result is not None:
                result["key"] = key_result
            return ActionResult(
                success=True,
                stdout=json.dumps(result, ensure_ascii=False) + "\n",
                data={"command": "certify", **result},
            )
        except Exception as exc:
            return ActionResult(success=False, error=str(exc), exit_code=1)

    def _do_scan(
        self,
        ctx: RequestContext,
        *,
        skill_dir: str | None = None,
        all_skills: bool = False,
        scanner_names: list[str] | None = None,
        force: bool = False,
        **kw: Any,
    ) -> ActionResult:
        try:
            if all_skills:
                if skill_dir is not None:
                    return ActionResult(
                        success=False,
                        error="--all and skill_dir are mutually exclusive.",
                        exit_code=1,
                    )
                dirs = resolve_skill_dirs()
                if not dirs:
                    return ActionResult(
                        success=False,
                        error="No skill directories found in config.json",
                        exit_code=1,
                    )
                key_created, key_result = self._ensure_keys()
                backend = NativeEd25519Backend()
                results = scan_batch(
                    dirs,
                    backend,
                    scanner_names=scanner_names,
                    force=force,
                )
                has_error = any(r.get("status") == "error" for r in results)
                data = {
                    "command": "scan",
                    "keyCreated": key_created,
                    "results": results,
                }
                if key_result is not None:
                    data["key"] = key_result
                return ActionResult(
                    success=not has_error,
                    stdout=json.dumps(data, ensure_ascii=False) + "\n",
                    data=data,
                    exit_code=1 if has_error else 0,
                )
            if skill_dir is None:
                return ActionResult(
                    success=False,
                    error="skill_dir is required (or use --all)",
                    exit_code=1,
                )
            key_created, key_result = self._ensure_keys()
            backend = NativeEd25519Backend()
            result = scan_skill(
                skill_dir,
                backend,
                scanner_names=scanner_names,
                force=force,
            )
            result["keyCreated"] = key_created
            if key_result is not None:
                result["key"] = key_result
            return ActionResult(
                success=True,
                stdout=json.dumps(result, ensure_ascii=False) + "\n",
                data={"command": "scan", **result},
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
                    "autoInvocable": s.enabled and s.type == "builtin",
                    "description": s.description,
                }
            )

        data = {"command": "list-scanners", "scanners": scanner_data}
        return ActionResult(
            success=True,
            stdout=json.dumps(data, ensure_ascii=False) + "\n",
            data=data,
        )
