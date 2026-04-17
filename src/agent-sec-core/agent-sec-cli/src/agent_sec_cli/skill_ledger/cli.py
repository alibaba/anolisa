"""skill-ledger CLI — Typer subcommand group.

Mounted as a subcommand group under ``agent-sec-cli skill-ledger <cmd>``.
All commands route through ``invoke("skill_ledger", command=..., ...)``
to participate in the middleware lifecycle (tracing, event logging, error handling).
"""

import getpass
import os
from typing import Optional

import typer
from agent_sec_cli.security_middleware import invoke

app = typer.Typer(
    name="skill-ledger",
    help="Skill change-tracking, integrity verification, and tamper-proof signing.",
    add_completion=True,
)


# ---------------------------------------------------------------------------
# Shared helper
# ---------------------------------------------------------------------------


def _forward(result) -> None:
    """Print ActionResult stdout/error and exit with its exit_code."""
    if result.stdout:
        typer.echo(result.stdout, nl=False)
    if result.error:
        typer.echo(result.error, err=True)
    raise typer.Exit(code=result.exit_code)


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
    """Generate Ed25519 signing key pair."""
    # Resolve passphrase: env-var > --passphrase flag > None
    passphrase: str | None = None
    env_pass = os.environ.get("SKILL_LEDGER_PASSPHRASE")
    if env_pass:
        passphrase = env_pass
    elif use_passphrase:
        passphrase = getpass.getpass("Enter passphrase for new signing key: ")
        confirm = getpass.getpass("Confirm passphrase: ")
        if passphrase != confirm:
            typer.echo("Error: passphrases do not match", err=True)
            raise typer.Exit(code=1)
        if not passphrase:
            typer.echo("Error: passphrase cannot be empty", err=True)
            raise typer.Exit(code=1)

    result = invoke(
        "skill_ledger", command="init-keys", force=force, passphrase=passphrase
    )
    _forward(result)


# ---------------------------------------------------------------------------
# check
# ---------------------------------------------------------------------------


@app.command("check")
def cmd_check(
    skill_dir: str = typer.Argument(..., help="Path to skill directory"),
) -> None:
    """Check skill integrity status (used by hooks)."""
    result = invoke("skill_ledger", command="check", skill_dir=skill_dir)
    _forward(result)


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
    """Create or update a signed manifest with scan findings."""
    scanner_names = [s.strip() for s in scanners.split(",")] if scanners else None
    result = invoke(
        "skill_ledger",
        command="certify",
        skill_dir=skill_dir,
        all_skills=all_skills,
        findings=findings,
        scanner=scanner,
        scanner_version=scanner_version,
        scanner_names=scanner_names,
    )
    _forward(result)


# ---------------------------------------------------------------------------
# status
# ---------------------------------------------------------------------------


@app.command("status")
def cmd_status(
    skill_dir: str = typer.Argument(..., help="Path to skill directory"),
) -> None:
    """Show human-readable skill status (for debugging)."""
    result = invoke("skill_ledger", command="status", skill_dir=skill_dir)
    _forward(result)


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
    result = invoke(
        "skill_ledger",
        command="audit",
        skill_dir=skill_dir,
        verify_snapshots=verify_snapshots,
    )
    _forward(result)


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
# Main entry (for direct module invocation: python -m ...)
# ---------------------------------------------------------------------------


def main() -> None:
    """Main entry point for the ``skill-ledger`` CLI."""
    app()


if __name__ == "__main__":
    main()
