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
    help=(
        "Skill security management — track changes, verify integrity, and sign skills.\n\n"
        "Typical workflow:\n\n"
        "  1. init-keys  Generate signing key pair (one-time setup)\n"
        "  2. check      Verify a skill's integrity status\n"
        "  3. certify    Record scan findings and sign the manifest\n"
        "  4. status     View a human-readable summary\n"
        "  5. audit      Deep-verify the full version history\n\n"
        "Integrity statuses:\n\n"
        "  pass      Files unchanged, signature valid, scan clean\n"
        "  none      Never scanned — baseline will be created on first check\n"
        "  drifted   Skill files changed since last certification\n"
        "  warn      Scan found low-risk issues\n"
        "  deny      Scan found high-risk issues\n"
        "  tampered  Manifest signature verification failed"
    ),
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
    force: bool = typer.Option(
        False, "--force", help="Overwrite existing keys (old key pair is archived)"
    ),
    use_passphrase: bool = typer.Option(
        False,
        "--passphrase",
        help="Protect the private key with an interactive passphrase (or set SKILL_LEDGER_PASSPHRASE env var for CI)",
    ),
) -> None:
    """Generate an Ed25519 signing key pair (one-time setup).

    Creates a key pair used to sign skill manifests. Run this once before
    using any other skill-ledger command.

    Key storage:
      ~/.local/share/skill-ledger/key.enc  (encrypted private key, 0600)
      ~/.local/share/skill-ledger/key.pub  (public key, 0644)

    By default, no passphrase is required — safe for non-interactive use.
    """
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
    skill_dir: str = typer.Argument(..., help="Path to the skill directory to check"),
) -> None:
    """Check a skill's integrity and output its security status as JSON.

    Compares current file hashes against the signed manifest and verifies
    the digital signature. Possible statuses:

      pass      Files unchanged, signature valid, scan clean
      none      Never scanned — a baseline manifest is created automatically
      drifted   Skill files changed since last certification
      warn      Signature valid, but scan found low-risk issues
      deny      Signature valid, but scan found high-risk issues
      tampered  Manifest signature verification failed — possible forgery
    """
    result = invoke("skill_ledger", command="check", skill_dir=skill_dir)
    _forward(result)


# ---------------------------------------------------------------------------
# certify
# ---------------------------------------------------------------------------


@app.command("certify")
def cmd_certify(
    skill_dir: Optional[str] = typer.Argument(
        None, help="Path to the skill directory (omit when using --all)"
    ),
    findings: Optional[str] = typer.Option(
        None,
        "--findings",
        help="Path to a findings JSON file from an external scanner (e.g., skill-vetter)",
    ),
    scanner: str = typer.Option(
        "skill-vetter",
        "--scanner",
        help="Name of the scanner that produced the findings file",
    ),
    scanner_version: Optional[str] = typer.Option(
        None,
        "--scanner-version",
        help="Version of the scanner that produced the findings",
    ),
    scanners: Optional[str] = typer.Option(
        None,
        "--scanners",
        help="Comma-separated scanner names to auto-invoke (e.g., 'skill-vetter,custom')",
    ),
    all_skills: bool = typer.Option(
        False,
        "--all",
        help="Certify every skill listed in ~/.config/skill-ledger/config.json skillDirs",
    ),
) -> None:
    """Record scan findings into a signed manifest for a skill.

    Two input modes:

      External findings (recommended for Agent-driven scans):
        certify <dir> --findings <file> --scanner skill-vetter

      Auto-invoke (run registered scanners automatically):
        certify <dir> --scanners <names>

    What certify does:
      1. Verify file consistency (creates a new version if files changed)
      2. Normalize findings and merge into the manifest scans[]
      3. Aggregate scanStatus (pass / warn / deny)
      4. Re-sign and write to .skill-meta/latest.json
    """
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
    skill_dir: str = typer.Argument(..., help="Path to the skill directory"),
) -> None:
    """Display a human-readable summary of a skill's security state.

    Shows the current integrity status, version ID, scan results, and
    file-change details in a readable format. Useful for quick inspection
    without parsing JSON (unlike 'check', which outputs machine-readable JSON).
    """
    result = invoke("skill_ledger", command="status", skill_dir=skill_dir)
    _forward(result)


# ---------------------------------------------------------------------------
# audit
# ---------------------------------------------------------------------------


@app.command("audit")
def cmd_audit(
    skill_dir: str = typer.Argument(..., help="Path to the skill directory to audit"),
    verify_snapshots: bool = typer.Option(
        False,
        "--verify-snapshots",
        help="Also verify that snapshot file hashes match stored records",
    ),
) -> None:
    """Verify the full version-chain integrity for a skill.

    Walks every historical version in .skill-meta/versions/ and checks:

      - Hash consistency (file hashes match the recorded values)
      - Signature validity (each version's digital signature is correct)
      - Chain linkage (each version references the previous signature)

    Use --verify-snapshots to additionally validate snapshot file hashes
    against the stored records — useful for detecting silent file corruption.
    """
    result = invoke(
        "skill_ledger",
        command="audit",
        skill_dir=skill_dir,
        verify_snapshots=verify_snapshots,
    )
    _forward(result)


# ---------------------------------------------------------------------------
# list-scanners
# ---------------------------------------------------------------------------


@app.command("list-scanners")
def cmd_list_scanners() -> None:
    """List registered scanners and their configuration.

    Shows all scanners defined in the built-in defaults and
    ~/.config/skill-ledger/config.json, including their invocation type,
    result parser, and enabled status.

    Use this to discover valid values for the --scanner flag in certify.
    """
    from agent_sec_cli.skill_ledger.scanner.registry import ScannerRegistry

    registry = ScannerRegistry.from_config()
    scanners = registry.list_scanners(enabled_only=False)
    if not scanners:
        typer.echo("No scanners registered.")
        raise typer.Exit(code=0)

    typer.echo(f"{'NAME':<20} {'TYPE':<10} {'PARSER':<18} {'ENABLED':<8} DESCRIPTION")
    for s in scanners:
        typer.echo(
            f"{s.name:<20} {s.type:<10} {s.parser:<18} "
            f"{'yes' if s.enabled else 'no':<8} {s.description}"
        )
    raise typer.Exit(code=0)


# ---------------------------------------------------------------------------
# set-policy (stub)
# ---------------------------------------------------------------------------


@app.command("set-policy", hidden=True)
def cmd_set_policy(
    skill_dir: str = typer.Argument(..., help="Path to the skill directory"),
    policy: str = typer.Option(
        ..., "--policy", help="Execution policy to apply: allow | block | warning"
    ),
) -> None:
    """Set a skill's execution policy (coming soon).

    Will control whether a skill is allowed to run, blocked, or triggers a
    warning based on its security state. Not yet implemented.
    """
    typer.echo("set-policy: this feature is coming soon.")
    raise typer.Exit(code=0)


# ---------------------------------------------------------------------------
# rotate-keys (stub)
# ---------------------------------------------------------------------------


@app.command("rotate-keys", hidden=True)
def cmd_rotate_keys() -> None:
    """Rotate the signing key pair (coming soon).

    Will archive the current key pair and generate a new one, allowing
    continued verification of manifests signed with the old keys.
    """
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
