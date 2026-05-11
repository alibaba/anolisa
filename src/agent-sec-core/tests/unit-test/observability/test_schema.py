"""Unit tests for observability record payload validation."""

import pytest
from agent_sec_cli.observability.metrics import HOOK_METRIC_ALLOWLIST
from agent_sec_cli.observability.schema import ObservabilityRecord
from pydantic import ValidationError

MINIMAL_METRICS_BY_HOOK = {
    "before_agent_run": {"prompt": "Summarize ./README.md"},
    "before_context_assembly": {"system_prompt": "You are a concise assistant."},
    "before_llm_call": {"model_id": "gpt-example"},
    "after_llm_response": {"input_tokens": 12},
    "before_tool_call": {"tool_name": "read_file"},
    "after_tool_call": {"result": {"ok": True}},
    "after_agent_run": {"response": "Done."},
}


def _payload(**overrides):
    payload = {
        "schemaVersion": 1,
        "hook": "before_agent_run",
        "observedAt": "2026-05-11T12:00:00Z",
        "metadata": {
            "sessionId": "session-123",
            "runId": "run-123",
        },
        "metrics": {"prompt": "Summarize ./README.md"},
    }
    payload.update(overrides)
    return payload


def test_minimal_metric_examples_cover_each_hook():
    assert set(MINIMAL_METRICS_BY_HOOK) == set(HOOK_METRIC_ALLOWLIST)


@pytest.mark.parametrize(("hook", "metrics"), MINIMAL_METRICS_BY_HOOK.items())
def test_each_hook_accepts_minimal_allowed_metric(hook, metrics):
    record = ObservabilityRecord.model_validate(_payload(hook=hook, metrics=metrics))

    assert record.schema_version == 1
    assert record.hook == hook
    assert record.metrics == metrics
    assert record.metadata.session_id == "session-123"
    assert record.metadata.run_id == "run-123"
    assert record.observed_at.tzinfo is not None


def test_camel_case_payload_dumps_back_to_wire_aliases():
    record = ObservabilityRecord.model_validate(_payload())

    dumped = record.model_dump(by_alias=True)

    assert "schemaVersion" in dumped
    assert "observedAt" in dumped
    assert dumped["metadata"]["sessionId"] == "session-123"
    assert dumped["metadata"]["runId"] == "run-123"


def test_all_allowed_metrics_are_not_required():
    record = ObservabilityRecord.model_validate(
        _payload(
            hook="before_agent_run",
            metrics={"prompt_length_tokens": 12},
        )
    )

    assert record.metrics == {"prompt_length_tokens": 12}


def test_missing_session_id_is_allowed():
    record = ObservabilityRecord.model_validate(_payload(metadata={"runId": "run-123"}))

    assert record.metadata.session_id is None
    assert record.metadata.run_id == "run-123"


def test_missing_run_id_is_allowed():
    record = ObservabilityRecord.model_validate(
        _payload(metadata={"sessionId": "session-123"})
    )

    assert record.metadata.session_id == "session-123"
    assert record.metadata.run_id is None


@pytest.mark.parametrize("field_name", ("sessionId", "runId"))
def test_empty_session_id_or_run_id_is_allowed(field_name):
    metadata = {"sessionId": "session-123", "runId": "run-123"}
    metadata[field_name] = ""

    record = ObservabilityRecord.model_validate(_payload(metadata=metadata))

    if field_name == "sessionId":
        assert record.metadata.session_id == ""
    else:
        assert record.metadata.run_id == ""


def test_unknown_hook_fails():
    with pytest.raises(ValidationError, match="unknown observability hook"):
        ObservabilityRecord.model_validate(_payload(hook="during_agent_run"))


def test_unknown_metric_fails():
    with pytest.raises(ValidationError, match="unknown metric"):
        ObservabilityRecord.model_validate(
            _payload(metrics={"prompt": "ok", "unlisted_metric": 1})
        )


def test_empty_metrics_fails():
    with pytest.raises(ValidationError, match="at least one allowed metric"):
        ObservabilityRecord.model_validate(_payload(metrics={}))


def test_invalid_timestamp_fails():
    with pytest.raises(ValidationError):
        ObservabilityRecord.model_validate(_payload(observedAt="not-a-timestamp"))


def test_naive_timestamp_fails():
    with pytest.raises(ValidationError, match="timezone-aware"):
        ObservabilityRecord.model_validate(_payload(observedAt="2026-05-11T12:00:00"))
