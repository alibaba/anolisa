"""Pydantic schema for ``observability record`` payloads."""

from datetime import datetime
from typing import Any, Literal

from agent_sec_cli.observability.metrics import (
    HOOK_METRIC_ALLOWLIST,
    allowed_metrics_for_hook,
)
from pydantic import (
    BaseModel,
    ConfigDict,
    Field,
    field_validator,
    model_validator,
)

UNKNOWN_HOOK_ERROR = "unknown observability hook"
UNKNOWN_METRIC_ERROR = "unknown metric"
EMPTY_METRICS_ERROR = "metrics must include at least one allowed metric"
NAIVE_TIMESTAMP_ERROR = "observedAt must be timezone-aware"


class ObservabilityMetadata(BaseModel):
    """Correlation metadata required on every observability record."""

    model_config = ConfigDict(populate_by_name=True, extra="allow")

    session_id: str | None = Field(default=None, alias="sessionId")
    run_id: str | None = Field(default=None, alias="runId")


class ObservabilityRecord(BaseModel):
    """Validated wire payload for ``agent-sec-cli observability record``."""

    model_config = ConfigDict(populate_by_name=True, extra="forbid")

    schema_version: Literal[1] = Field(alias="schemaVersion")
    hook: str
    observed_at: datetime = Field(alias="observedAt")
    metadata: ObservabilityMetadata
    metrics: dict[str, Any]

    @field_validator("hook")
    @classmethod
    def _validate_hook(cls, value: str) -> str:
        if value not in HOOK_METRIC_ALLOWLIST:
            raise ValueError(
                f"{UNKNOWN_HOOK_ERROR} {value!r}; "
                f"expected one of {sorted(HOOK_METRIC_ALLOWLIST)}"
            )
        return value

    @field_validator("observed_at")
    @classmethod
    def _validate_observed_at(cls, value: datetime) -> datetime:
        if value.tzinfo is None or value.tzinfo.utcoffset(value) is None:
            raise ValueError(NAIVE_TIMESTAMP_ERROR)
        return value

    @model_validator(mode="after")
    def _validate_metrics(self) -> "ObservabilityRecord":
        allowed_metrics = allowed_metrics_for_hook(self.hook)
        if not self.metrics:
            raise ValueError(f"{EMPTY_METRICS_ERROR} for hook {self.hook!r}")

        unknown_metrics = sorted(set(self.metrics) - allowed_metrics)
        if unknown_metrics:
            raise ValueError(
                f"{UNKNOWN_METRIC_ERROR}(s) for hook {self.hook!r}: "
                f"{unknown_metrics}; allowed metrics are {sorted(allowed_metrics)}"
            )

        return self
