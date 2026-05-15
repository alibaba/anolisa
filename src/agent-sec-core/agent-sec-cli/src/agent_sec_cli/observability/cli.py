"""Typer commands for observability ingestion."""

import json
import sys
from typing import Any

import typer
from agent_sec_cli.observability import record_observability
from agent_sec_cli.observability.schema import (
    ObservabilityRecord,
    observability_record_json_schema,
    validate_observability_record,
)
from pydantic import ValidationError

app = typer.Typer(help="Record observability metrics.")

_INPUT_FORMAT = "json"


class ObservabilityCliError(ValueError):
    """User-facing observability CLI validation error."""


def _validation_message(exc: ValidationError) -> str:
    errors = exc.errors()
    if not errors:
        return str(exc)
    message = str(errors[0].get("msg", exc))
    return message.removeprefix("Value error, ")


def _parse_record(value: Any) -> ObservabilityRecord:
    if not isinstance(value, dict):
        raise ObservabilityCliError("payload must be a JSON object")
    try:
        return validate_observability_record(value)
    except ValidationError as exc:
        raise ObservabilityCliError(_validation_message(exc)) from exc


def _parse_json(raw: str) -> ObservabilityRecord:
    if not raw.strip():
        raise ObservabilityCliError("stdin is empty")
    try:
        return _parse_record(json.loads(raw))
    except json.JSONDecodeError as exc:
        raise ObservabilityCliError(f"invalid JSON: {exc.msg}") from exc


@app.command()
def record(
    format_: str = typer.Option("json", "--format", help="Input format: json."),
    use_stdin: bool = typer.Option(False, "--stdin", help="Read payload from stdin."),
) -> None:
    """Record one observability JSON object from stdin.

    Required wire fields: hook, observedAt, metadata, metrics.
    Unknown top-level fields, metadata fields, and metric keys are ignored for
    forward compatibility. If no supported metrics remain, the record is rejected.
    """
    if format_ != _INPUT_FORMAT:
        typer.echo("Error: --format must be json.", err=True)
        raise typer.Exit(code=1)

    if not use_stdin:
        typer.echo("Error: --stdin is required.", err=True)
        raise typer.Exit(code=1)

    raw = sys.stdin.read()
    try:
        record_payload = _parse_json(raw)
    except ObservabilityCliError as exc:
        typer.echo(f"Error: {exc}", err=True)
        raise typer.Exit(code=1)

    try:
        record_observability(record_payload)
    except Exception as exc:  # noqa: BLE001
        typer.echo(f"Error: failed to write observability record: {exc}", err=True)
        raise typer.Exit(code=1) from exc
    raise typer.Exit(code=0)


@app.command(name="schema")
def schema_command() -> None:
    """Print the public observability record JSON Schema."""
    typer.echo(
        json.dumps(
            observability_record_json_schema(),
            indent=2,
            ensure_ascii=False,
        )
    )


__all__ = ["app"]
