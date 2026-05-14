"""CLI entry point for the PII checker (scan-pii command)."""

from pathlib import Path
from typing import Any

import typer
from agent_sec_cli.security_middleware import invoke

scanner_app = typer.Typer(
    name="scan-pii",
    help=(
        "Detect PII and credentials, printing verdict, findings, and summary. "
        "Use --redact-output to also emit redacted_text."
    ),
)

_OUTPUT_FORMATS = {"json", "text"}
_SOURCES = {"user_input", "tool_output", "manual", "unknown"}
_DEFAULT_MAX_BYTES = 1_048_576
_TEXT_OPTION = typer.Option(None, "--text", help="Text to scan.")
_INPUT_OPTION = typer.Option(
    None,
    "--input",
    exists=True,
    file_okay=True,
    dir_okay=False,
    readable=True,
    resolve_path=True,
    help="Path to a UTF-8 text file to scan.",
)
_FORMAT_OPTION = typer.Option("json", "--format", help="Output format: json or text.")
_INCLUDE_LOW_OPTION = typer.Option(
    False,
    "--include-low-confidence",
    help="Include findings below the default confidence threshold.",
)
_RAW_EVIDENCE_OPTION = typer.Option(
    False,
    "--raw-evidence",
    help=(
        "Include raw evidence in local CLI output for debugging. "
        "Raw evidence is never written to security events."
    ),
)
_REDACT_OUTPUT_OPTION = typer.Option(
    False,
    "--redact-output",
    help=(
        "Also include redacted_text in output. "
        "This does not rewrite input files or local data."
    ),
)
_SOURCE_OPTION = typer.Option(
    "unknown",
    "--source",
    help=(
        "Audit and policy context label: user_input, tool_output, manual, or "
        "unknown. This does not modify input content."
    ),
)
_MAX_BYTES_OPTION = typer.Option(
    _DEFAULT_MAX_BYTES,
    "--max-bytes",
    help="Maximum bytes to scan before truncating input.",
)


def _read_limited_input(path: Path, max_bytes: int) -> tuple[str, bool, int]:
    """Read at most max_bytes from a UTF-8 file before scanner-level limiting.

    The returned byte count reflects file bytes read for scanning. When the CLI
    truncates a file, that truncation flag takes precedence over the scanner's
    string-level truncation result in the final summary.
    """
    with path.open("rb") as handle:
        data = handle.read(max_bytes + 1)
    truncated = len(data) > max_bytes
    if truncated:
        data = data[:max_bytes]
    return data.decode("utf-8", errors="ignore"), truncated, len(data)


def _format_text_output(data: dict[str, Any]) -> str:
    """Render a scan result as human-readable text without raw evidence."""
    lines = [
        f"Verdict: {data.get('verdict', 'unknown')}",
        f"Findings: {data.get('summary', {}).get('total', 0)}",
    ]
    summary = data.get("summary", {})
    if isinstance(summary, dict):
        source = summary.get("source")
        if source:
            lines.append(f"Source: {source}")

    findings = data.get("findings", [])
    if isinstance(findings, list) and findings:
        lines.append("")
        for finding in findings:
            if not isinstance(finding, dict):
                continue
            lines.append(
                "- {type} ({severity}, confidence={confidence}): {evidence}".format(
                    type=finding.get("type", "unknown"),
                    severity=finding.get("severity", "unknown"),
                    confidence=finding.get("confidence", "?"),
                    evidence=finding.get("evidence_redacted", "[REDACTED]"),
                )
            )

    redacted_text = data.get("redacted_text")
    if isinstance(redacted_text, str):
        lines.extend(["", "Redacted text:", redacted_text])

    return "\n".join(lines) + "\n"


@scanner_app.callback(invoke_without_command=True)
def scan_pii(
    ctx: typer.Context,
    text: str | None = _TEXT_OPTION,
    input_path: Path | None = _INPUT_OPTION,
    output_format: str = _FORMAT_OPTION,
    include_low_confidence: bool = _INCLUDE_LOW_OPTION,
    raw_evidence: bool = _RAW_EVIDENCE_OPTION,
    redact_output: bool = _REDACT_OUTPUT_OPTION,
    source: str = _SOURCE_OPTION,
    max_bytes: int = _MAX_BYTES_OPTION,
) -> None:
    """Detect PII and credentials in text or a file."""
    if ctx.invoked_subcommand is not None:
        return
    if output_format not in _OUTPUT_FORMATS:
        typer.echo("Error: --format must be one of: json, text.", err=True)
        raise typer.Exit(code=1)
    if source not in _SOURCES:
        typer.echo(
            "Error: --source must be one of: user_input, tool_output, manual, unknown.",
            err=True,
        )
        raise typer.Exit(code=1)
    if max_bytes <= 0:
        typer.echo("Error: --max-bytes must be greater than zero.", err=True)
        raise typer.Exit(code=1)
    if (text is None and input_path is None) or (
        text is not None and input_path is not None
    ):
        typer.echo("Error: provide exactly one of --text or --input.", err=True)
        raise typer.Exit(code=1)

    input_truncated = False
    input_bytes_scanned = None
    scan_text = text or ""
    if input_path is not None:
        scan_text, input_truncated, input_bytes_scanned = _read_limited_input(
            input_path, max_bytes
        )

    result = invoke(
        "pii_scan",
        text=scan_text,
        source=source,
        include_low_confidence=include_low_confidence,
        raw_evidence=raw_evidence,
        redact_output=redact_output,
        max_bytes=max_bytes,
        input_truncated=input_truncated,
        input_bytes_scanned=input_bytes_scanned,
    )

    if output_format == "json":
        typer.echo(result.stdout)
    else:
        typer.echo(_format_text_output(result.data), nl=False)

    if result.error:
        typer.echo(result.error, err=True)
    raise typer.Exit(code=result.exit_code)
