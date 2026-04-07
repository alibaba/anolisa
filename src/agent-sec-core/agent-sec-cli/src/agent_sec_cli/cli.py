"""CLI entry point for agent-sec-cli package."""

import sys
import os

import typer

# Ensure security_middleware is importable
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))

from agent_sec_cli.security_middleware import invoke

app = typer.Typer(
    name="agent-sec-cli",
    help="AgentSecCore unified CLI entry point",
    add_completion=True,
)


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


@app.command()
def harden(
    mode: str = typer.Option(
        "scan",
        "--mode",
        help="Hardening mode (default: scan)",
        case_sensitive=False,
    ),
    config: str = typer.Option(
        "agentos_baseline",
        "--config",
        help="Hardening config baseline (default: agentos_baseline)",
    ),
):
    """System security hardening."""
    # Validate mode choices
    if mode not in ["scan", "reinforce", "dry-run"]:
        typer.echo(f"Error: Invalid mode '{mode}'. Choose from: scan, reinforce, dry-run", err=True)
        raise typer.Exit(code=1)
    
    result = invoke("harden", mode=mode, config=config)
    if result.stdout:
        typer.echo(result.stdout)
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


def main():
    """Main entry point."""
    app()


if __name__ == "__main__":
    main()
