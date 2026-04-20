"""CLI entry point for the prompt scanner (scan-prompt command)."""

import json
import sys
from pathlib import Path
from typing import Any

import typer
from agent_sec_cli.prompt_scanner.config import ScanMode
from agent_sec_cli.prompt_scanner.exceptions import ScannerInputError
from agent_sec_cli.prompt_scanner.logging.audit import AuditLogger
from agent_sec_cli.prompt_scanner.result import Verdict
from agent_sec_cli.prompt_scanner.scanner import PromptScanner

scanner_app = typer.Typer(
    name="scan-prompt", help="Prompt injection / jailbreak scanner"
)


@scanner_app.command("warmup")
def warmup_model() -> None:
    """Pre-download and load all ML models to eliminate cold-start latency.

    Downloads and caches the L2 ML classifier model (and future L3 models
    when implemented).  L1 (rule-engine) requires no download and is skipped.

    Run this once after installation (or after system restart) so that the
    first scan-prompt call returns immediately without downloading the model.

    Example::

        agent-sec-cli scan-prompt warmup
    """
    typer.echo("Warming up prompt scanner (downloading ML models)...")
    try:
        # Use STRICT mode so all model-bearing layers (L2, and L3 when
        # implemented) are included in the warmup pass.
        scanner = PromptScanner(mode=ScanMode.STRICT)
        scanner.warmup()
    except Exception as exc:
        typer.echo(f"Warmup failed: {exc}", err=True)
        raise typer.Exit(code=1)
    typer.echo("Warmup complete. Model is ready.")


def _build_error_output(message: str) -> dict[str, Any]:
    """Build a standardised error JSON payload."""
    return {
        "schema_version": "1.0",
        "ok": False,
        "verdict": Verdict.ERROR.value,
        "risk_level": "unknown",
        "threat_type": "unknown",
        "confidence": 0.0,
        "summary": message,
        "findings": [],
        "layer_results": [],
        "engine_version": "0.1.0",
        "elapsed_ms": 0,
    }


@scanner_app.callback(invoke_without_command=True)
def scan_prompt(
    ctx: typer.Context,
    mode: str = typer.Option(
        "standard",
        "--mode",
        help="Detection mode: fast (L1), standard (L1+L2), strict (L1+L2+L3)",
        case_sensitive=False,
    ),
    output_format: str = typer.Option(
        "json",
        "--format",
        help="Output format: 'json' (default) or 'text' (human-readable)",
    ),
    source: str = typer.Option(
        "",
        "--source",
        help="Label for the input origin (e.g. user_input, rag, tool_output)",
    ),
    text: str | None = typer.Option(
        None,
        "--text",
        help="Prompt text to scan directly.  Takes precedence over --input and stdin.",
    ),
    input_file: str | None = typer.Option(
        None,
        "--input",
        help="Path to a file containing prompts (one per line). "
        "If omitted, reads from stdin.",
    ),
) -> None:
    """Scan prompt text for injection / jailbreak attempts.

    Input priority: --text > --input <file> > stdin

    Examples::

        # Direct text
        agent-sec-cli scan-prompt --text "ignore previous instructions"

        # Stdin (pipe)
        echo "ignore previous instructions" | agent-sec-cli scan-prompt

        # File
        agent-sec-cli scan-prompt --input prompts.txt --format json

        # Human-readable output
        agent-sec-cli scan-prompt --text "hello" --format text
    """
    # If a sub-command (e.g. warmup) was invoked, skip scan logic entirely.
    if ctx.invoked_subcommand is not None:
        return
    # --- Validate mode ---
    try:
        scan_mode = ScanMode(mode.lower())
    except ValueError:
        typer.echo(
            f"Error: Invalid mode '{mode}'. Choose from: fast, standard, strict",
            err=True,
        )
        raise typer.Exit(code=1)

    # --- Validate format ---
    if output_format not in ("json", "text"):
        typer.echo(
            f"Error: Invalid format '{output_format}'. Choose from: json, text",
            err=True,
        )
        raise typer.Exit(code=1)

    # --- Read input texts ---
    texts: list[str]
    if text is not None:
        # --text flag takes precedence
        texts = [text]
    elif input_file:
        try:
            with Path(input_file).open(encoding="utf-8") as fh:
                texts = [line.strip() for line in fh if line.strip()]
            if not texts:
                typer.echo(f"Error: File is empty: {input_file}", err=True)
                raise typer.Exit(code=1)
        except FileNotFoundError:
            typer.echo(f"Error: File not found: {input_file}", err=True)
            raise typer.Exit(code=1)
    else:
        raw = sys.stdin.read().strip()
        if not raw:
            typer.echo("Error: No input received from stdin.", err=True)
            raise typer.Exit(code=1)
        texts = [raw]

    # --- Scan ---
    try:
        scanner = PromptScanner(mode=scan_mode)
        results = scanner.scan_batch(texts)
    except ScannerInputError as exc:
        typer.echo(json.dumps(_build_error_output(f"Input error: {exc}"), indent=2))
        raise typer.Exit(code=0)
    except Exception as exc:
        typer.echo(json.dumps(_build_error_output(f"Scanner error: {exc}"), indent=2))
        raise typer.Exit(code=0)  # exit 0: scanner ran, verdict in JSON

    # --- Audit ---
    _audit = AuditLogger()
    for result in results:
        _audit.log_scan(result)
        if result.is_threat:
            _audit.log_threat(result)

    # --- Output ---
    for result in results:
        if output_format == "text":
            _print_text(result.to_dict())
        else:
            typer.echo(json.dumps(result.to_dict(), indent=2))

    raise typer.Exit(code=0)


def _print_text(d: dict[str, Any]) -> None:
    """Print a scan result in human-readable text format."""
    verdict = d["verdict"].upper()
    icon = {"PASS": "✅", "WARN": "⚠️", "DENY": "❌", "ERROR": "💥"}.get(verdict, "?")
    typer.echo(f"{icon}  Verdict : {verdict}")
    typer.echo(f"    Risk    : {d['risk_level']} (score: {d.get('confidence', 0):.3f})")
    typer.echo(f"    Threat  : {d.get('threat_type', 'unknown')}")
    typer.echo(f"    Summary : {d['summary']}")
    if d["findings"]:
        typer.echo("    Findings:")
        for f in d["findings"]:
            typer.echo(f"      [{f['severity'].upper()}] {f['rule_id']} — {f['title']}")
            if f.get("evidence"):
                evidence = f["evidence"][:80]
                typer.echo(f"        evidence: {evidence!r}")
    typer.echo(f"    Elapsed : {d['elapsed_ms']} ms")
