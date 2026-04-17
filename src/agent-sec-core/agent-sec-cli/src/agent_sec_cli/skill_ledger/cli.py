"""skill-ledger CLI — Typer application with all subcommands.

Entry point:  ``skill-ledger = "agent_sec_cli.skill_ledger.cli:main"``
"""

import json
import logging
from typing import Optional

import typer

from agent_sec_cli.skill_ledger.errors import SkillLedgerError

logger = logging.getLogger(__name__)

app = typer.Typer(
    name="skill-ledger",
    help="Skill change-tracking, integrity verification, and tamper-proof signing.",
    add_completion=True,
)


# ---------------------------------------------------------------------------
# Shared helpers
# ---------------------------------------------------------------------------


def _get_backend():
    """Instantiate the configured signing backend (currently always Ed25519)."""
    from agent_sec_cli.skill_ledger.signing.ed25519 import NativeEd25519Backend

    return NativeEd25519Backend()


def _emit_event(event_type: str, category: str, details: dict) -> None:
    """Fire-and-forget security event logging (integration with security_events)."""
    try:
        from agent_sec_cli.security_events import SecurityEvent, log_event

        log_event(
            SecurityEvent(
                event_type=event_type,
                category=category,
                details=details,
            )
        )
    except Exception:  # noqa: BLE001
        logger.debug("Failed to emit security event %r", event_type, exc_info=True)


def _json_output(data: dict) -> None:
    """Print compact JSON to stdout (hook-compatible one-liner)."""
    typer.echo(json.dumps(data, ensure_ascii=False))


# ---------------------------------------------------------------------------
# init-keys
# ---------------------------------------------------------------------------


@app.command("init-keys")
def cmd_init_keys(
    force: bool = typer.Option(False, "--force", help="Overwrite existing keys"),
    use_passphrase: bool = typer.Option(
        False, "--passphrase", help="Encrypt the private key with a passphrase"
    ),
) -> None:
    """Generate Ed25519 signing key pair.

    By default the private key is stored **unencrypted** (no interactive
    prompt).  Pass ``--passphrase`` to encrypt it, or set the
    ``SKILL_LEDGER_PASSPHRASE`` environment variable.
    """
    import os

    from agent_sec_cli.skill_ledger.signing.key_manager import ensure_keys_not_exist

    try:
        ensure_keys_not_exist(force=force)
    except SkillLedgerError as exc:
        typer.echo(str(exc), err=True)
        raise typer.Exit(code=1)

    # Resolve passphrase: env-var > --passphrase flag > None (no encryption)
    passphrase: str | None = None
    env_pass = os.environ.get("SKILL_LEDGER_PASSPHRASE")
    if env_pass:
        passphrase = env_pass
    elif use_passphrase:
        import getpass

        passphrase = getpass.getpass("Enter passphrase for new signing key: ")
        confirm = getpass.getpass("Confirm passphrase: ")
        if passphrase != confirm:
            typer.echo("Error: passphrases do not match", err=True)
            raise typer.Exit(code=1)
        if not passphrase:
            typer.echo("Error: passphrase cannot be empty", err=True)
            raise typer.Exit(code=1)

    backend = _get_backend()
    try:
        result = backend.generate_keys(passphrase)
    except Exception as exc:
        typer.echo(f"Error generating keys: {exc}", err=True)
        raise typer.Exit(code=1)

    _emit_event("skill_ledger_init_keys", "skill_ledger", {"fingerprint": result["fingerprint"]})
    _json_output(result)


# ---------------------------------------------------------------------------
# check
# ---------------------------------------------------------------------------


@app.command("check")
def cmd_check(
    skill_dir: str = typer.Argument(..., help="Path to skill directory"),
) -> None:
    """Check skill integrity status (used by hooks)."""
    from agent_sec_cli.skill_ledger.core.checker import check

    backend = _get_backend()
    try:
        result = check(skill_dir, backend)
    except SkillLedgerError as exc:
        _json_output({"status": "error", "error": str(exc)})
        raise typer.Exit(code=1)

    _emit_event("skill_ledger_check", "skill_ledger", result)
    _json_output(result)

    # Non-zero exit for security-critical states so callers can gate on $?
    status = result.get("status", "")
    if status in ("tampered", "deny"):
        raise typer.Exit(code=1)


# ---------------------------------------------------------------------------
# certify
# ---------------------------------------------------------------------------


@app.command("certify")
def cmd_certify(
    skill_dir: Optional[str] = typer.Argument(None, help="Path to skill directory"),
    findings: Optional[str] = typer.Option(
        None, "--findings", help="Path to findings JSON file (external mode)"
    ),
    scanner: str = typer.Option(
        "skill-vetter", "--scanner", help="Scanner identifier (used with --findings)"
    ),
    scanner_version: str = typer.Option(
        "0.1.0", "--scanner-version", help="Scanner version"
    ),
    scanners: Optional[str] = typer.Option(
        None, "--scanners", help="Comma-separated scanner names for auto-invoke mode"
    ),
    all_skills: bool = typer.Option(
        False, "--all", help="Certify all skills from config.json skillDirs"
    ),
) -> None:
    """Create or update a signed manifest with scan findings.

    Two input modes:

    - With --findings: read an external findings file (e.g. from skill-vetter).

    - Without --findings: auto-invoke registered non-skill scanners.
      (In v1 only skill-vetter is registered; auto-invoke has no effect.)
    """
    from agent_sec_cli.skill_ledger.core.certifier import certify, certify_batch

    backend = _get_backend()
    scanner_names = [s.strip() for s in scanners.split(",")] if scanners else None

    try:
        if all_skills:
            from agent_sec_cli.skill_ledger.config import resolve_skill_dirs

            dirs = resolve_skill_dirs()
            if not dirs:
                typer.echo("No skill directories found in config.json", err=True)
                raise typer.Exit(code=1)
            results = certify_batch(
                dirs,
                backend,
                findings_path=findings,
                scanner=scanner,
                scanner_version=scanner_version,
                scanner_names=scanner_names,
            )
            _emit_event("skill_ledger_certify_batch", "skill_ledger", {"results": results})
            _json_output({"results": results})
        else:
            if skill_dir is None:
                typer.echo("Error: skill_dir is required (or use --all)", err=True)
                raise typer.Exit(code=1)
            result = certify(
                skill_dir,
                backend,
                findings_path=findings,
                scanner=scanner,
                scanner_version=scanner_version,
                scanner_names=scanner_names,
            )
            _emit_event("skill_ledger_certify", "skill_ledger", result)
            _json_output(result)
    except SkillLedgerError as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(code=1)


# ---------------------------------------------------------------------------
# status
# ---------------------------------------------------------------------------


@app.command("status")
def cmd_status(
    skill_dir: str = typer.Argument(..., help="Path to skill directory"),
) -> None:
    """Show human-readable skill status (for debugging)."""
    from agent_sec_cli.skill_ledger.core.checker import check
    from agent_sec_cli.skill_ledger.core.version_chain import (
        list_version_ids,
        load_latest_manifest,
    )

    backend = _get_backend()

    # Run check
    try:
        result = check(skill_dir, backend)
    except SkillLedgerError as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(code=1)

    status = result.get("status", "unknown")
    skill_name = _skill_name(skill_dir)

    typer.echo(f"Skill:      {skill_name}")
    typer.echo(f"Directory:  {skill_dir}")
    typer.echo(f"Status:     {status}")

    manifest = load_latest_manifest(skill_dir)
    if manifest is not None:
        typer.echo(f"Version:    {manifest.versionId}")
        typer.echo(f"scanStatus: {manifest.scanStatus}")
        typer.echo(f"Policy:     {manifest.policy}")
        typer.echo(f"Scans:      {len(manifest.scans)}")
        typer.echo(f"Files:      {len(manifest.fileHashes)}")
        if manifest.signature is not None:
            typer.echo(f"Signed by:  {manifest.signature.keyFingerprint}")

    versions = list_version_ids(skill_dir)
    typer.echo(f"Versions:   {len(versions)}")

    if status == "drifted":
        typer.echo(f"  Added:    {result.get('added', [])}")
        typer.echo(f"  Removed:  {result.get('removed', [])}")
        typer.echo(f"  Modified: {result.get('modified', [])}")
    elif status == "tampered":
        typer.echo(f"  Reason:   {result.get('reason', '')}")


# ---------------------------------------------------------------------------
# audit
# ---------------------------------------------------------------------------


@app.command("audit")
def cmd_audit(
    skill_dir: str = typer.Argument(..., help="Path to skill directory"),
    verify_snapshots: bool = typer.Option(
        False, "--verify-snapshots", help="Also verify snapshot file hashes"
    ),
) -> None:
    """Deep-verify version chain integrity."""
    from agent_sec_cli.skill_ledger.core.auditor import audit

    backend = _get_backend()

    try:
        result = audit(skill_dir, backend, verify_snapshots=verify_snapshots)
    except SkillLedgerError as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(code=1)

    _emit_event("skill_ledger_audit", "skill_ledger", result)
    _json_output(result)

    if not result["valid"]:
        raise typer.Exit(code=1)


# ---------------------------------------------------------------------------
# set-policy (stub)
# ---------------------------------------------------------------------------


@app.command("set-policy")
def cmd_set_policy(
    skill_dir: str = typer.Argument(..., help="Path to skill directory"),
    policy: str = typer.Option(
        ..., "--policy", help="Execution policy: allow | block | warning"
    ),
) -> None:
    """Set skill execution policy (coming soon)."""
    typer.echo("set-policy: this feature is coming soon.")
    raise typer.Exit(code=0)


# ---------------------------------------------------------------------------
# rotate-keys (stub)
# ---------------------------------------------------------------------------


@app.command("rotate-keys")
def cmd_rotate_keys() -> None:
    """Rotate signing keys (coming soon)."""
    typer.echo("rotate-keys: this feature is coming soon.")
    raise typer.Exit(code=0)


# ---------------------------------------------------------------------------
# Internal helpers
# ---------------------------------------------------------------------------


def _skill_name(skill_dir: str) -> str:
    from pathlib import Path

    return Path(skill_dir).name


# ---------------------------------------------------------------------------
# Main entry
# ---------------------------------------------------------------------------


def main() -> None:
    """Main entry point for the ``skill-ledger`` CLI."""
    app()


if __name__ == "__main__":
    main()
