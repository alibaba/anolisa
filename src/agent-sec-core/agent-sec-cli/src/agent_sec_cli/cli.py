"""CLI entry point for agent-sec-cli package."""

import typer
from agent_sec_cli.prompt_scanner.cli import scanner_app
from agent_sec_cli.security_middleware import invoke
from agent_sec_cli.security_middleware.backends.hardening import (
    DEFAULT_HARDEN_CONFIG,
)
from agent_sec_cli.skill_ledger.cli import app as skill_ledger_app

app = typer.Typer(
    name="agent-sec-cli",
    help="AgentSecCore unified CLI entry point",
    add_completion=True,
)

# Mount skill-ledger as a subcommand group: agent-sec-cli skill-ledger <cmd>
app.add_typer(skill_ledger_app, name="skill-ledger")

_HARDEN_HELP_TEXT = f"""\
Usage: agent-sec-cli harden [SEHARDEN_ARGS]...

Defaults:
  If omitted, the wrapper adds `--scan --config {DEFAULT_HARDEN_CONFIG}`.

Examples:
  agent-sec-cli harden --scan --config {DEFAULT_HARDEN_CONFIG}
  agent-sec-cli harden --reinforce --config {DEFAULT_HARDEN_CONFIG}
  agent-sec-cli harden --reinforce --dry-run --config {DEFAULT_HARDEN_CONFIG}

Common SEHarden flags:
  --scan              Run compliance scan.
  --reinforce         Apply remediation actions.
  --dry-run           Preview reinforce actions without changing the system.
  --config <ruleset>  Select a profile name or YAML file.
  --level <level>     Limit execution to a profile level.
  --verbose           Show detailed rule-level evidence.
  --log-level <lv>    Set log level: trace|debug|info|warn|error.

Help:
  agent-sec-cli harden --help             Show this concise wrapper help.
  agent-sec-cli harden --downstream-help  Show full `loongshield seharden` help.
"""


def _with_default_harden_args(args: list[str]) -> list[str]:
    """Add wrapper defaults when the caller does not provide them explicitly."""
    normalized = list(args)
    if (
        "--scan" not in normalized
        and "--reinforce" not in normalized
        and "--dry-run" not in normalized
    ):
        normalized.insert(0, "--scan")
    if "--config" not in normalized and not any(
        arg.startswith("--config=") for arg in normalized
    ):
        normalized.extend(["--config", DEFAULT_HARDEN_CONFIG])
    return normalized


# Register prompt scanner sub-command
app.add_typer(scanner_app, name="scan-prompt")


@app.command(name="log-sandbox", hidden=True)
def log_sandbox(
    decision: str = typer.Option(
        "",
        "--decision",
        help="Sandbox decision (allow/block/sandbox)",
    ),
    command: str = typer.Option(
        "",
        "--command",
        help="Command being evaluated",
    ),
    reasons: str = typer.Option(
        "",
        "--reasons",
        help="Reasons for the decision",
    ),
    network_policy: str = typer.Option(
        "",
        "--network-policy",
        help="Network policy applied",
    ),
    cwd: str = typer.Option(
        "",
        "--cwd",
        help="Current working directory",
    ),
):
    """Internal: Record sandbox prehook decision (called by sandbox-guard.py)."""
    result = invoke(
        "sandbox_prehook",
        decision=decision,
        command=command,
        reasons=reasons,
        network_policy=network_policy,
        cwd=cwd,
    )
    # Silent exit - async call doesn't need output
    raise typer.Exit(code=result.exit_code)


@app.command(
    short_help="Scan or reinforce the system against a security baseline.",
    context_settings={
        "allow_extra_args": True,
        "ignore_unknown_options": True,
        "help_option_names": [],
    },
)
def harden(
    ctx: typer.Context,
    help_flag: bool = typer.Option(
        False,
        "--help",
        "-h",
        is_eager=True,
        help="Show concise harden help and examples.",
    ),
    downstream_help: bool = typer.Option(
        False,
        "--downstream-help",
        help="Show full `loongshield seharden` help and exit.",
    ),
):
    """Scan or reinforce the system against a security baseline."""
    if help_flag:
        typer.echo(_HARDEN_HELP_TEXT.rstrip())
        raise typer.Exit(code=0)

    if downstream_help:
        result = invoke("harden", args=["--help"])
    else:
        result = invoke("harden", args=_with_default_harden_args(list(ctx.args)))

    if result.stdout:
        typer.echo(result.stdout, nl=False)
    if result.error:
        typer.echo(result.error, err=True)
    raise typer.Exit(code=result.exit_code)


@app.command()
def verify(
    skill: str = typer.Option(
        None,
        "--skill",
        help="Path to specific skill for verification",
    ),
):
    """Skill integrity verification."""
    result = invoke("verify", skill=skill)
    if result.stdout:
        typer.echo(result.stdout)
    if result.error:
        typer.echo(result.error, err=True)
    raise typer.Exit(code=result.exit_code)


@app.command(name="code-scan")
def code_scan(
    code: str = typer.Option("", "--code", help="Source code to scan"),
    language: str = typer.Option("bash", "--language", help="Language: bash or python"),
) -> None:
    """Scan code for security issues."""
    if not code.strip():
        typer.echo("Error: --code is required (use --code '<source>')", err=True)
        raise typer.Exit(code=1)
    result = invoke("code_scan", code=code, language=language)
    if result.stdout:
        typer.echo(result.stdout)
    if result.error:
        typer.echo(result.error, err=True)
    raise typer.Exit(code=result.exit_code)


def main():
    """Main entry point."""
    app()


if __name__ == "__main__":
    main()
