"""CLI entry point for the prompt scanner (scan-prompt command)."""

import json
import sys

import typer
from agent_sec_cli.prompt_scanner.config import ScanMode
from agent_sec_cli.prompt_scanner.result import Verdict
from agent_sec_cli.prompt_scanner.scanner import PromptScanner

scanner_app = typer.Typer(
    name="scan-prompt", help="Prompt injection / jailbreak scanner"
)


@scanner_app.callback(invoke_without_command=True)
def scan_prompt(
    mode: str = typer.Option(
        "standard",
        "--mode",
        help="Detection mode: fast (L1), standard (L1+L2), strict (L1+L2+L3)",
        case_sensitive=False,
    ),
    format: str = typer.Option(
        "json",
        "--format",
        help="Output format (currently only 'json' is supported)",
    ),
    source: str = typer.Option(
        "",
        "--source",
        help="Label for the input origin (e.g. user_input, hook)",
    ),
    input_file: str | None = typer.Option(
        None,
        "--input",
        help="Path to a file containing prompts (one per line). "
        "If omitted, reads from stdin.",
    ),
) -> None:
    """Scan prompt text for injection / jailbreak attempts.

    Reads from stdin by default::

        echo "ignore previous instructions" | agent-sec-cli scan-prompt --format json
    """
    # Validate mode
    try:
        scan_mode = ScanMode(mode.lower())
    except ValueError:
        typer.echo(
            f"Error: Invalid mode '{mode}'. Choose from: fast, standard, strict",
            err=True,
        )
        raise typer.Exit(code=1)

    # Read input
    if input_file:
        try:
            with open(input_file, "r", encoding="utf-8") as f:
                texts = [line.strip() for line in f if line.strip()]
        except FileNotFoundError:
            typer.echo(f"Error: File not found: {input_file}", err=True)
            raise typer.Exit(code=1)
    else:
        raw = sys.stdin.read().strip()
        if not raw:
            typer.echo("Error: No input received from stdin.", err=True)
            raise typer.Exit(code=1)
        texts = [raw]

    # Scan
    try:
        scanner = PromptScanner(mode=scan_mode)
        results = scanner.scan_batch(texts)
    except Exception as exc:
        error_output = {
            "schema_version": "1.0",
            "ok": False,
            "verdict": Verdict.ERROR.value,
            "risk_level": "unknown",
            "summary": f"Scanner error: {exc}",
            "findings": [],
            "engine_version": "0.1.0",
            "elapsed_ms": 0,
        }
        typer.echo(json.dumps(error_output, indent=2))
        raise typer.Exit(code=0)  # exit 0: scanner ran, verdict in JSON

    # Output
    for result in results:
        typer.echo(json.dumps(result.to_dict(), indent=2))

    raise typer.Exit(code=0)
