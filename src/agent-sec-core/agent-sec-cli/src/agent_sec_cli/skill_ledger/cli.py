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
from agent_sec_cli.security_middleware.result import ActionResult

app = typer.Typer(
    name="skill-ledger",
    help=(
        "Skill security management — track changes, verify integrity, and sign skills.\n\n"
        "Typical workflow:\n\n"
        "  1. init      Initialize keys and baseline covered skills\n"
        "  2. scan      Run built-in scanners and sign the manifest\n"
        "  3. check     Verify a skill's integrity status\n"
        "  4. certify   Import external findings and sign the manifest\n"
        "  5. status    Show overall ledger health overview\n"
        "  6. audit     Deep-verify the full version history\n\n"
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


def _forward(result: ActionResult) -> None:
    """Print ActionResult stdout/error and exit with its exit_code."""
    if result.stdout:
        typer.echo(result.stdout, nl=False)
    if result.error:
        typer.echo(result.error, err=True)
    raise typer.Exit(code=result.exit_code)


def _parse_scanner_names(scanners: Optional[str]) -> list[str] | None:
    """Parse a comma-separated scanner list."""
    return [s.strip() for s in scanners.split(",") if s.strip()] if scanners else None


def _resolve_new_key_passphrase(use_passphrase: bool) -> str | None:
    """Resolve --passphrase using env first, then interactive prompts."""
    passphrase: str | None = None
    if use_passphrase:
        env_pass = os.environ.get("SKILL_LEDGER_PASSPHRASE")
        if env_pass is not None:
            # Use ``is not None`` so that SKILL_LEDGER_PASSPHRASE="" is
            # accepted (treated as "no passphrase" — unencrypted keys).
            passphrase = env_pass if env_pass else None
        else:
            passphrase = getpass.getpass("Enter passphrase for new signing key: ")
            confirm = getpass.getpass("Confirm passphrase: ")
            if passphrase != confirm:
                typer.echo("Error: passphrases do not match", err=True)
                raise typer.Exit(code=1)
            if not passphrase:
                typer.echo("Error: passphrase cannot be empty", err=True)
                raise typer.Exit(code=1)
    return passphrase


# ---------------------------------------------------------------------------
# init
# ---------------------------------------------------------------------------


@app.command("init")
def cmd_init(
    no_baseline: bool = typer.Option(
        False,
        "--no-baseline",
        help="Only initialize keys; do not scan covered skills.",
    ),
    use_passphrase: bool = typer.Option(
        False,
        "--passphrase",
        help="Protect a newly-created private key with a passphrase.",
    ),
    force_keys: bool = typer.Option(
        False,
        "--force-keys",
        help="Overwrite existing keys (old public key is archived).",
    ),
    scanners: Optional[str] = typer.Option(
        None,
        "--scanners",
        help="Comma-separated built-in scanners for baseline scan (default: code-scanner,static-scanner).",
    ),
) -> None:
    """Initialize skill-ledger and baseline covered skills."""
    passphrase = _resolve_new_key_passphrase(use_passphrase)
    result = invoke(
        "skill_ledger",
        command="init",
        baseline=not no_baseline,
        passphrase=passphrase,
        force_keys=force_keys,
        scanner_names=_parse_scanner_names(scanners),
    )
    _forward(result)


# ---------------------------------------------------------------------------
# init-keys (hidden compatibility command)
# ---------------------------------------------------------------------------


@app.command("init-keys", hidden=True)
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
      ~/.local/share/agent-sec/skill-ledger/key.enc  (encrypted private key, 0600)
      ~/.local/share/agent-sec/skill-ledger/key.pub  (public key, 0644)

    By default, no passphrase is required — safe for non-interactive use.
    """
    passphrase = _resolve_new_key_passphrase(use_passphrase)
    result = invoke(
        "skill_ledger", command="init-keys", force=force, passphrase=passphrase
    )
    _forward(result)


# ---------------------------------------------------------------------------
# check
# ---------------------------------------------------------------------------


@app.command("check")
def cmd_check(
    skill_dir: Optional[str] = typer.Argument(
        None, help="Path to the skill directory to check (omit when using --all)"
    ),
    all_skills: bool = typer.Option(
        False,
        "--all",
        help="Check every registered skill at once.",
    ),
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

    Use --all to check every registered skill and receive a JSON array of
    enriched results. Skill discovery uses built-in default directories plus
    ~/.config/agent-sec/skill-ledger/config.json managedSkillDirs (paths and
    globs expanded automatically by the CLI). Set enableDefaultSkillDirs=false
    in config.json for isolated runs that should ignore built-in defaults.
    """
    if all_skills and skill_dir is not None:
        typer.echo(
            "Error: --all and skill_dir are mutually exclusive.",
            err=True,
        )
        raise typer.Exit(code=1)

    result = invoke(
        "skill_ledger",
        command="check",
        skill_dir=skill_dir,
        all_skills=all_skills,
    )
    _forward(result)


# ---------------------------------------------------------------------------
# scan
# ---------------------------------------------------------------------------


@app.command("scan")
def cmd_scan(
    skill_dir: Optional[str] = typer.Argument(
        None, help="Path to the skill directory to scan (omit when using --all)"
    ),
    all_skills: bool = typer.Option(
        False,
        "--all",
        help="Scan every registered skill using fill-in behavior.",
    ),
    force: bool = typer.Option(
        False,
        "--force",
        help="Re-run requested scanners even when matching results already exist.",
    ),
    scanners: Optional[str] = typer.Option(
        None,
        "--scanners",
        help="Comma-separated built-in scanner names (default: code-scanner,static-scanner).",
    ),
) -> None:
    """Run built-in scanners and record signed scan results."""
    if all_skills and skill_dir is not None:
        typer.echo(
            "Error: --all and skill_dir are mutually exclusive.",
            err=True,
        )
        raise typer.Exit(code=1)

    result = invoke(
        "skill_ledger",
        command="scan",
        skill_dir=skill_dir,
        all_skills=all_skills,
        force=force,
        scanner_names=_parse_scanner_names(scanners),
    )
    _forward(result)


# ---------------------------------------------------------------------------
# certify
# ---------------------------------------------------------------------------


@app.command("certify")
def cmd_certify(
    skill_dir: str = typer.Argument(..., help="Path to the skill directory"),
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
    delete_findings: bool = typer.Option(
        False,
        "--delete-findings",
        help="Delete the findings file after a successful import.",
    ),
) -> None:
    """Import external scanner findings into a signed manifest."""
    if findings is None:
        typer.echo(
            "Error: --findings is required for certify. Use 'skill-ledger scan' for built-in scanners.",
            err=True,
        )
        raise typer.Exit(code=1)

    result = invoke(
        "skill_ledger",
        command="certify",
        skill_dir=skill_dir,
        findings=findings,
        scanner=scanner,
        scanner_version=scanner_version,
        delete_findings=delete_findings,
    )
    _forward(result)


# ---------------------------------------------------------------------------
# status
# ---------------------------------------------------------------------------


@app.command("status")
def cmd_status(
    verbose: bool = typer.Option(
        False,
        "--verbose",
        "-v",
        help="Include per-skill results array in the output.",
    ),
) -> None:
    """Show an overview of the skill-ledger system health.

    Reports signing key infrastructure, configuration state, and
    aggregate integrity status across all registered skills.

    Output is a single JSON object with three sections:

      keys     Signing key status (initialized, fingerprint, encrypted)
      config   Configuration summary (default/managed skill dirs, scanners)
      skills   Aggregate health (discovered count, per-status breakdown)

    Use --verbose to include the full per-skill results array.
    For per-skill integrity checks use the 'check' command instead.
    """
    result = invoke(
        "skill_ledger",
        command="status",
        verbose=verbose,
    )
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
    ~/.config/agent-sec/skill-ledger/config.json, including their invocation type,
    result parser, and enabled status.

    Use this to discover valid values for scan --scanners and certify --scanner.
    """
    result = invoke("skill_ledger", command="list-scanners")
    _forward(result)


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
