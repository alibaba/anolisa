"""CLI entry point for the prompt scanner (scan-prompt command)."""

import os
import sys
from pathlib import Path
from typing import Any

import typer
from agent_sec_cli.correlation_context import get_current_trace_context
from agent_sec_cli.daemon.client import DaemonClient
from agent_sec_cli.daemon.errors import DaemonClientError
from agent_sec_cli.daemon.protocol import DaemonResponse
from agent_sec_cli.prompt_scanner.config import ScanMode
from agent_sec_cli.prompt_scanner.result import Verdict
from agent_sec_cli.prompt_scanner.scanner import PromptScanner

scanner_app = typer.Typer(
    name="scan-prompt", help="Prompt injection / jailbreak scanner"
)
DAEMON_REQUEST_TIMEOUT_MS = 30_000
DAEMON_DISABLED_ENV = "AGENT_SEC_DAEMON_DISABLED"


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
        # Current behaviour: each non-empty line in the file is treated as an
        # independent prompt and scanned separately.  This is intentionally
        # designed for bulk red-team testing where a test corpus lists one
        # attack payload per line.
        #
        # TODO: add a --input-full / --whole-file flag (or auto-detect via a
        #       future --input-mode={lines,whole} option) so that the entire
        #       file content is treated as a single prompt.  That mode is
        #       needed when scanning a complete RAG document, a conversation
        #       transcript, or any multi-paragraph text stored in a file.
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

    # --- Scan through daemon (daemon owns prompt model runtime and audit writes) ---
    # Each text is scanned individually so that every invocation gets its own
    # daemon request and SecurityEvent record.  This ensures precise per-input
    # auditability: when a threat is detected, the audit log pinpoints exactly
    # which input triggered it.  Batching would collapse multiple inputs into a
    # single trace_id, losing that granularity without any performance benefit:
    # under STANDARD/STRICT mode scan_batch() is sequential anyway because the
    # HuggingFace tokenizer (Rust-backed, uses RefCell internally) is NOT
    # thread-safe — all inference is serialised behind _inference_lock.
    for t in texts:
        try:
            response = _call_scan_prompt_daemon(t, scan_mode.value, source)
        except DaemonClientError as exc:
            typer.echo(_daemon_unavailable_message(str(exc)), err=True)
            raise typer.Exit(code=1)

        if not response.ok:
            typer.echo(response.stderr or _daemon_error_message(response), err=True)
            raise typer.Exit(code=response.exit_code or 1)

        # --- Output ---
        if output_format == "text":
            if not response.data:
                typer.echo(f"Error: {response.stderr}", err=True)
                raise typer.Exit(code=response.exit_code)
            _print_text(response.data)
        else:
            typer.echo(response.stdout)

    raise typer.Exit(code=0)


def _call_scan_prompt_daemon(
    text: str,
    mode: str,
    source: str,
) -> DaemonResponse:
    """Call the daemon scan-prompt method with CLI-resolved params."""
    if _daemon_disabled():
        raise DaemonClientError(
            f"{DAEMON_DISABLED_ENV}=1 is set; scan-prompt requires the daemon"
        )

    return DaemonClient(timeout_ms=DAEMON_REQUEST_TIMEOUT_MS).call(
        "scan-prompt",
        params={"text": text, "mode": mode, "source": source},
        trace_context=_current_trace_context_payload(),
        timeout_ms=DAEMON_REQUEST_TIMEOUT_MS,
    )


def _current_trace_context_payload() -> dict[str, str]:
    ctx = get_current_trace_context()
    if ctx is None:
        return {}

    payload: dict[str, str] = {}
    for field_name in (
        "trace_id",
        "session_id",
        "run_id",
        "call_id",
        "tool_call_id",
    ):
        value = getattr(ctx, field_name)
        if value is not None:
            payload[field_name] = value
    return payload


def _daemon_disabled() -> bool:
    raw_value = os.environ.get(DAEMON_DISABLED_ENV, "").strip().lower()
    return raw_value in {"1", "true", "yes", "on"}


def _daemon_unavailable_message(detail: str) -> str:
    return (
        "Error: agent-sec daemon is unavailable for scan-prompt. "
        "Start agent-sec-core.service and retry. "
        f"Detail: {detail}"
    )


def _daemon_error_message(response: DaemonResponse) -> str:
    if response.error:
        return response.error.get("message", "daemon request failed")
    return "daemon request failed"


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
            typer.echo(f"      {f['rule_id']} — {f['title']}")
            if f.get("evidence"):
                evidence = f["evidence"][:80]
                typer.echo(f"        evidence: {evidence!r}")
    typer.echo(f"    Elapsed : {d['elapsed_ms']} ms")
