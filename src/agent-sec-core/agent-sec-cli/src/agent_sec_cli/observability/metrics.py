"""Hook metric allowlist for observability record payloads."""

from collections.abc import Mapping

HOOK_METRIC_ALLOWLIST: Mapping[str, frozenset[str]] = {
    "before_agent_run": frozenset(
        {
            "prompt",
            "prompt_length_chars",
            "prompt_length_tokens",
            "encoding_anomalies",
            "contains_url",
            "contains_file_path",
            "contains_code_snippet",
        }
    ),
    "before_context_assembly": frozenset(
        {
            "system_prompt",
            "history_tokens",
            "context_window_utilization",
        }
    ),
    "before_llm_call": frozenset(
        {
            "model_id",
            "model_provider",
            "prompt",
            "history_messages_count",
        }
    ),
    "after_llm_response": frozenset(
        {
            "input_tokens",
            "response_tokens",
            "finish_reason",
            "contains_code",
            "contains_credentials",
            "contains_pii",
            "contains_urls",
            "tool_calls_count",
            "tool_calls",
        }
    ),
    "before_tool_call": frozenset(
        {
            "tool_name",
            "parameters",
        }
    ),
    "after_tool_call": frozenset(
        {
            "result",
            "error",
            "duration",
        }
    ),
    "after_agent_run": frozenset({"response"}),
}


def allowed_metrics_for_hook(hook: str) -> frozenset[str]:
    """Return the metric names allowed for *hook*, or an empty set."""
    return HOOK_METRIC_ALLOWLIST.get(hook, frozenset())
