"""Skill-ledger backend — dispatch to skill-ledger core operations.

Routes the ``command`` kwarg to the appropriate handler, returning a
unified :class:`ActionResult`.
"""

import json
from pathlib import Path
from typing import Any

from agent_sec_cli.security_middleware.backends.base import BaseBackend
from agent_sec_cli.security_middleware.context import RequestContext
from agent_sec_cli.security_middleware.result import ActionResult
from agent_sec_cli.skill_ledger.config import resolve_skill_dirs
from agent_sec_cli.skill_ledger.core.auditor import audit
from agent_sec_cli.skill_ledger.core.certifier import certify, certify_batch
from agent_sec_cli.skill_ledger.core.checker import check
from agent_sec_cli.skill_ledger.core.version_chain import (
    list_version_ids,
    load_latest_manifest,
)
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

        return ActionResult(
            success=True,
            stdout=json.dumps(result, ensure_ascii=False) + "\n",
            data={"command": "init-keys", **result},
        )

    def _do_check(
        self, ctx: RequestContext, *, skill_dir: str, **kw: Any
    ) -> ActionResult:
        backend = NativeEd25519Backend()
        try:
            result = check(skill_dir, backend)
        except Exception as exc:
            error_data = {"status": "error", "error": str(exc)}
            return ActionResult(
                success=False,
                stdout=json.dumps(error_data, ensure_ascii=False) + "\n",
                data={"command": "check", **error_data},
                exit_code=1,
            )

        status = result.get("status", "")
        is_critical = status in ("tampered", "deny")
        return ActionResult(
            success=not is_critical,
            stdout=json.dumps(result, ensure_ascii=False) + "\n",
            data={"command": "check", **result},
            exit_code=1 if is_critical else 0,
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
                data = {"command": "certify", "results": results}
                return ActionResult(
                    success=True,
                    stdout=json.dumps({"results": results}, ensure_ascii=False) + "\n",
                    data=data,
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
        self, ctx: RequestContext, *, skill_dir: str, **kw: Any
    ) -> ActionResult:
        backend = NativeEd25519Backend()

        try:
            result = check(skill_dir, backend)
        except Exception as exc:
            return ActionResult(success=False, error=str(exc), exit_code=1)

        status = result.get("status", "unknown")
        skill_name = Path(skill_dir).name

        lines = [
            f"Skill:      {skill_name}",
            f"Directory:  {skill_dir}",
            f"Status:     {status}",
        ]

        manifest = load_latest_manifest(skill_dir)
        if manifest is not None:
            lines.append(f"Version:    {manifest.versionId}")
            lines.append(f"scanStatus: {manifest.scanStatus}")
            lines.append(f"Policy:     {manifest.policy}")
            lines.append(f"Scans:      {len(manifest.scans)}")
            lines.append(f"Files:      {len(manifest.fileHashes)}")
            if manifest.signature is not None:
                lines.append(f"Signed by:  {manifest.signature.keyFingerprint}")

        versions = list_version_ids(skill_dir)
        lines.append(f"Versions:   {len(versions)}")

        if status == "drifted":
            lines.append(f"  Added:    {result.get('added', [])}")
            lines.append(f"  Removed:  {result.get('removed', [])}")
            lines.append(f"  Modified: {result.get('modified', [])}")
        elif status == "tampered":
            lines.append(f"  Reason:   {result.get('reason', '')}")

        return ActionResult(
            success=True,
            stdout="\n".join(lines) + "\n",
            data={"command": "status", **result},
        )

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

        if not scanners:
            return ActionResult(
                success=True,
                stdout="No scanners registered.\n",
                data={"command": "list-scanners", "scanners": []},
            )

        lines = [
            f"{'NAME':<20} {'TYPE':<10} {'PARSER':<18} {'ENABLED':<8} DESCRIPTION",
        ]
        scanner_data = []
        for s in scanners:
            lines.append(
                f"{s.name:<20} {s.type:<10} {s.parser:<18} "
                f"{'yes' if s.enabled else 'no':<8} {s.description}"
            )
            scanner_data.append(
                {
                    "name": s.name,
                    "type": s.type,
                    "parser": s.parser,
                    "enabled": s.enabled,
                    "description": s.description,
                }
            )

        return ActionResult(
            success=True,
            stdout="\n".join(lines) + "\n",
            data={"command": "list-scanners", "scanners": scanner_data},
        )
