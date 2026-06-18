"""CLI entry point for the prompt scanner (scan-prompt command)."""

import json
import sys
from pathlib import Path
from typing import Any

import typer
from agent_sec_cli.correlation_context import (
    get_current_trace_context,
    trace_context_to_payload,
)
from agent_sec_cli.daemon.client import DaemonClient
from agent_sec_cli.daemon.env import daemon_disabled
from agent_sec_cli.daemon.protocol import DaemonResponse
from agent_sec_cli.prompt_scanner.config import ScanMode
from agent_sec_cli.prompt_scanner.result import Verdict
from agent_sec_cli.prompt_scanner.scanner import PromptScanner
from agent_sec_cli.security_middleware import invoke

scanner_app = typer.Typer(
    name="scan-prompt", help="Prompt injection / jailbreak scanner"
)
DAEMON_REQUEST_TIMEOUT_MS = 30_000


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
        help="Detection mode: fast (L1), standard (L1+L2), strict (L1+L2+L3), multi_turn (L4, reads JSON from stdin)",
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

    For multi_turn (L4) mode, pipe a JSON payload via stdin with
    {history, current_query, assistant_response}:

        echo '{"history":[...],"current_query":"...","assistant_response":"..."}' | \\
            agent-sec-cli scan-prompt --mode multi_turn

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
            f"Error: Invalid mode '{mode}'. "
            "Choose from: fast, standard, strict, multi_turn",
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

    # --- MULTI_TURN mode: read JSON payload from stdin ---
    # L4 always invokes locally: it calls Ollama over HTTP directly, so the
    # daemon's L2 model pre-load gives no benefit, and the daemon's
    # prompt_scan_state readiness gate would reject a mode that doesn't need
    # the L2 model.  Audit parity is preserved — the local invoke() path
    # emits the same prompt_scan SecurityEvent as the daemon-disabled L1-L3
    # path (only the daemon access_log is skipped, identical to running with
    # AGENT_SEC_DAEMON_DISABLED=1).
    if scan_mode is ScanMode.MULTI_TURN:
        if text is not None or input_file:
            typer.echo(
                "Error: --text and --input are not supported with multi_turn mode. "
                "Pipe a JSON payload via stdin:\n"
                '  echo \'{"history":[...],"current_query":"...","assistant_response":"..."}\' | '
                "agent-sec-cli scan-prompt --mode multi_turn",
                err=True,
            )
            raise typer.Exit(code=1)

        raw = sys.stdin.read().strip()
        if not raw:
            typer.echo("Error: No input received from stdin.", err=True)
            raise typer.Exit(code=1)
        try:
            payload = json.loads(raw)
        except (json.JSONDecodeError, ValueError) as exc:
            typer.echo(f"Error: Invalid JSON: {exc}", err=True)
            raise typer.Exit(code=1)

        history = payload.get("history") or []
        current_query = payload.get("current_query") or ""
        assistant_response = payload.get("assistant_response") or ""
        if not isinstance(history, list) or not isinstance(current_query, str):
            typer.echo(
                "Error: payload must include a 'history' list and 'current_query' string.",
                err=True,
            )
            raise typer.Exit(code=1)
        if not current_query.strip():
            typer.echo("Error: current_query is empty.", err=True)
            raise typer.Exit(code=1)

        try:
            mw_result = invoke(
                "prompt_scan",
                text=current_query,
                mode=scan_mode.value,
                source=source,
                history=history,
                assistant_response=assistant_response,
            )
        except Exception as exc:
            typer.echo(
                json.dumps(
                    _build_error_output(f"Scanner error: {exc}"),
                    indent=2,
                    ensure_ascii=False,
                )
            )
            raise typer.Exit(code=1)

        # Warn when L4 is unavailable — the scan passes through with no
        # detectors having run (MULTI_TURN mode only configures L4).
        if mw_result.data and not mw_result.data.get("layer_results"):
            typer.echo(
                "Warning: L4 multi-turn intent detection is not available "
                "(Ollama unreachable). Scan returned a pass-through verdict.",
                err=True,
            )

        if output_format == "text":
            if not mw_result.data:
                typer.echo(f"Error: {mw_result.error}", err=True)
                raise typer.Exit(code=mw_result.exit_code)
            _print_text(mw_result.data)
        else:
            typer.echo(mw_result.stdout)
        raise typer.Exit(code=0)

    # --- Read input texts ---
    texts: list[str]
    if text is not None:
        # --text flag takes precedence
        if not text.strip():
            raise typer.Exit(code=0)
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

    use_daemon = _should_use_daemon()

    # --- Scan through daemon unless explicitly disabled, otherwise use local middleware ---
    # Each text is scanned individually so that every invocation gets its own
    # daemon request or local SecurityEvent record.  This ensures precise per-input
    # auditability: when a threat is detected, the audit log pinpoints exactly
    # which input triggered it.  Batching would collapse multiple inputs into a
    # single trace_id, losing that granularity without any performance benefit:
    # under STANDARD/STRICT mode scan_batch() is sequential anyway because the
    # HuggingFace tokenizer (Rust-backed, uses RefCell internally) is NOT
    # thread-safe — all inference is serialised behind _inference_lock.
    for t in texts:
        if use_daemon:
            try:
                response = _call_scan_prompt_daemon(t, scan_mode.value, source)
            except Exception as exc:
                _print_error_json(_daemon_unavailable_message(str(exc)))
                raise typer.Exit(code=0)

            if not response.ok:
                _print_error_json(response.stderr or _daemon_error_message(response))
                raise typer.Exit(code=0)

            daemon_exit_code = _print_daemon_response(response, output_format)
            if daemon_exit_code:
                raise typer.Exit(code=daemon_exit_code)
            continue

        try:
            mw_result = invoke(
                "prompt_scan",
                text=t,
                mode=scan_mode.value,
                source=source,
            )
        except Exception as exc:
            _print_error_json(f"Scanner error: {exc}")
            raise typer.Exit(code=0)

        # --- Output ---
        if output_format == "text":
            if not mw_result.data:
                typer.echo(f"Error: {mw_result.error}", err=True)
                raise typer.Exit(code=mw_result.exit_code or 1)
            _print_text(mw_result.data)
        else:
            if mw_result.stdout:
                typer.echo(mw_result.stdout)
            elif mw_result.data:
                typer.echo(json.dumps(mw_result.data, indent=2, ensure_ascii=False))
            else:
                _print_error_json(mw_result.error or "scan-prompt returned no output")

    raise typer.Exit(code=0)


def _call_scan_prompt_daemon(
    text: str,
    mode: str,
    source: str,
) -> DaemonResponse:
    """Call the daemon scan-prompt method with CLI-resolved params."""
    return DaemonClient(timeout_ms=DAEMON_REQUEST_TIMEOUT_MS).call(
        "scan-prompt",
        params={"text": text, "mode": mode, "source": source},
        trace_context=trace_context_to_payload(get_current_trace_context()),
        caller="cli",
        timeout_ms=DAEMON_REQUEST_TIMEOUT_MS,
    )


def _should_use_daemon() -> bool:
    """Return whether the CLI should try the daemon path for scan-prompt."""
    return not daemon_disabled()


def _daemon_unavailable_message(detail: str) -> str:
    return (
        "Error: agent-sec daemon is unavailable for scan-prompt. " f"Detail: {detail}"
    )


def _daemon_error_message(response: DaemonResponse) -> str:
    if response.error:
        return response.error.get("message", "daemon request failed")
    return "daemon request failed"


def _print_error_json(message: str) -> None:
    """Print a scanner-compatible ERROR verdict payload."""
    typer.echo(json.dumps(_build_error_output(message), indent=2, ensure_ascii=False))


def _print_daemon_response(response: DaemonResponse, output_format: str) -> int:
    """Print a successful daemon scan-prompt response and return a CLI exit code."""
    if output_format == "text":
        if response.data:
            _print_text(response.data)
            return 0
        typer.echo(
            f"Error: {response.stderr or 'scan-prompt returned no result data'}",
            err=True,
        )
        return response.exit_code or 1

    if response.stdout:
        typer.echo(response.stdout)
    elif response.data:
        typer.echo(json.dumps(response.data, indent=2, ensure_ascii=False))
    else:
        _print_error_json(
            response.stderr
            or f"scan-prompt returned no output (exit code {response.exit_code})"
        )
    return 0


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
