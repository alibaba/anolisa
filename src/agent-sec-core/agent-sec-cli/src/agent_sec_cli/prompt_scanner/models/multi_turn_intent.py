"""Multi-turn intent classifier (L4) — calls model service for classification.

L4 requires a running Ollama instance with the target model loaded.  When Ollama
is unavailable, the detector is silently skipped and MULTI_TURN mode returns a
pass-through verdict. L4 is only invoked when the user explicitly selects --mode multi_turn.
"""

import logging
import math
import os
import re
import time
from typing import Any

from agent_sec_cli.model_service import ModelServiceClient, create_client

log = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Model name configuration
# ---------------------------------------------------------------------------

_MODEL_NAME_ENV = "AGENT_SEC_OLLAMA_MODEL"
_DEFAULT_MODEL_NAME = "warden"


# ---------------------------------------------------------------------------
# Qwen3 chat template tokens (for raw mode)
# ---------------------------------------------------------------------------
# The warden model was fine-tuned with Qwen3's chat template applied.
# When using raw mode, we must wrap the prompt manually.
_CHAT_TEMPLATE_PREFIX = "<|im_start|>user\n"
_CHAT_TEMPLATE_SUFFIX = "<|im_end|>\n<|im_start|>assistant\n"

# Qwen3 special tokens that delineate chat turns.  If any of these survive
# into the raw prompt, an attacker can break out of the ``user`` role and
# forge fake ``assistant`` turns — e.g. pre-filling "1" (benign) to bias
# the classifier's logprobs.  All user-controlled text is stripped of
# these tokens before being inserted into the template.
_QWEN_SPECIAL_TOKEN_RE = re.compile(
    r"<\|im_start\|>|<\|im_end\|>|<\|endoftext\|>",
    re.IGNORECASE,
)


def _sanitize_special_tokens(text: str) -> str:
    """Remove Qwen3 chat-structural tokens from user-controlled text.

    See ``_QWEN_SPECIAL_TOKEN_RE`` for the attack rationale.
    """
    return _QWEN_SPECIAL_TOKEN_RE.sub("", str(text))


# ---------------------------------------------------------------------------
# Prompt template
# ---------------------------------------------------------------------------
# The warden model was trained with rewards landing on the
# "0"/"1" token immediately after this exact prompt; any drift from the
# upstream wording silently miscalibrates the logprobs softmax.
NAIVE_PROMPT_TEMPLATE_WITH_RESPONSE = """You are a safety classifier. Given the conversation below, determine if the assistant's response produces harmful content (violates safety, ethics, or legal guidelines) or is benign (safe).

Conversation history:
{history}

Current exchange:
USER: {question}
ASSISTANT: {assistant_response}

Does this exchange produce harmful content? You MUST respond with ONLY a single digit and nothing else: 0 for harmful, 1 for benign. Do not include any explanation or analysis."""


def _get_model_name() -> str:
    """Return the model name from environment or default."""
    return os.environ.get(_MODEL_NAME_ENV, _DEFAULT_MODEL_NAME)


def _normalize_role(role: Any) -> str:
    """Map a raw role value to a canonical uppercase label."""
    role = str(role).strip().lower()
    if role == "user":
        return "USER"
    if role == "assistant":
        return "ASSISTANT"
    if role == "system":
        return "SYSTEM"
    return "UNKNOWN"


def format_history(history: list[dict[str, Any] | str]) -> str:
    """Format prior turns into the ``USER: ...`` / ``ASSISTANT: ...`` block.

    Keeps the most recent ``_MAX_HISTORY_TURNS`` turns.  Empty history
    collapses to a placeholder string so the template still renders.

    Accepts two formats:
    - Canonical dict format: ``[{"role": "user", "content": "..."}]``
    - Legacy string format: ``["user: ...", "assistant: ..."]``
    """
    if not history:
        return "(No previous turns)"
    truncated = history[-_MAX_HISTORY_TURNS:]
    lines: list[str] = []
    for turn in truncated:
        if isinstance(turn, dict):
            role = _normalize_role(turn.get("role", "unknown"))
            content = _sanitize_special_tokens(turn.get("content", ""))
            lines.append(f"{role}: {content}")
        elif isinstance(turn, str):
            turn = _sanitize_special_tokens(turn)
            # Legacy format: "user: ..." or "assistant: ..."
            # Normalise the role prefix to uppercase for consistency
            # with the dict path above.
            if ": " in turn:
                role, _, content = turn.partition(": ")
                lines.append(f"{_normalize_role(role)}: {content}")
            else:
                lines.append(f"UNKNOWN: {turn}")
        else:
            lines.append(f"UNKNOWN: {_sanitize_special_tokens(turn)}")
    return "\n\n".join(lines)


# Single-pass placeholder matcher.  Using one re.sub sweep (instead of
# chained str.replace) guarantees that an attacker-controlled value
# containing a later placeholder literal — e.g. history text with
# "{question}" — is never re-interpreted as a template field.
_FIELD_RE = re.compile(r"\{(?:history|question|assistant_response)\}")


def format_defender_prompt(
    history: list[dict[str, Any] | str],
    current_query: str,
    assistant_response: str,
) -> str:
    """Format the user-content payload that gets wrapped in the chat template.

    Returns the full template-substituted string; the caller wraps it with
    Qwen3 chat-template tokens (``_CHAT_TEMPLATE_PREFIX`` / ``SUFFIX``) and
    sends the result via ``raw=True``.

    Uses a single-pass ``re.sub`` so attacker-controlled text in
    history/query/response is never re-interpreted as a template field — a
    risk with chained ``str.replace`` where an earlier value containing a
    later placeholder would be wrongly substituted.

    All user-controlled values are also stripped of Qwen3 special tokens
    (``<|im_start|>``, ``<|im_end|>``, ``<|endoftext|>``) to prevent
    prompt-injection via fake turn boundaries.
    """
    replacements = {
        "{history}": format_history(history),
        "{question}": _sanitize_special_tokens(current_query),
        "{assistant_response}": _sanitize_special_tokens(assistant_response),
    }
    return _FIELD_RE.sub(
        lambda m: replacements[m.group(0)], NAIVE_PROMPT_TEMPLATE_WITH_RESPONSE
    )


# ---------------------------------------------------------------------------
# Classifier
# ---------------------------------------------------------------------------

_DEFAULT_HARMFUL_THRESHOLD = 0.55

# Minimum combined probability mass that tokens "0" and "1" must carry
# for the 2-token softmax to be trustworthy.  Below this threshold the
# model has spread most of its probability over other vocabulary tokens
# (e.g. due to model drift away from the warden fine-tune), so the
# renormalised p_harmful is noise — we degrade to the text-based fallback
# and flag the result as low-confidence.
_MIN_TOTAL_PROB = 0.5

# Max recent turns forwarded to the classifier.  32 turns covers documented
# Crescendo attack chain lengths (<=5-10 rounds) with room for surrounding
# context.
_MAX_HISTORY_TURNS = 32


class MultiTurnIntentClassifier:
    """Classifies whether an assistant response is harmful (L4 multi-turn intent).

    Delegates HTTP calls to a ModelServiceClient; this class only handles
    prompt formatting and logprobs parsing.
    """

    def __init__(
        self,
        harmful_threshold: float = _DEFAULT_HARMFUL_THRESHOLD,
        client: ModelServiceClient | None = None,
    ) -> None:
        self._harmful_threshold = harmful_threshold
        self._model = _get_model_name()
        self._client: ModelServiceClient = client or create_client()

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def check_ready(self) -> bool:
        """Check if the model service is reachable and the target model is loaded.

        Returns True if the model is available, False on any error or if the
        model is absent.  This is used by ``MultiTurnIntentDetector.is_available()``
        to decide whether the L4 layer should be registered.
        """
        return self._client.check_model(self._model)

    def classify(
        self,
        history: list[dict[str, Any] | str],
        current_query: str,
        assistant_response: str,
    ) -> dict[str, Any]:
        """Classify one (history, query, response) triple.

        Returns dict with verdict ("block"/"pass"), p_harmful, latency_ms,
        history_turns, low_confidence.
        Raises RuntimeError if the model service is unreachable.
        """
        history_turns = len(history)

        prompt_body = format_defender_prompt(history, current_query, assistant_response)
        prompt = f"{_CHAT_TEMPLATE_PREFIX}{prompt_body}{_CHAT_TEMPLATE_SUFFIX}"

        t0 = time.perf_counter()
        body = self._client.generate(
            self._model,
            prompt,
            raw=True,
            logprobs=True,
            top_logprobs=10,
            options={"num_predict": 1, "temperature": 0},
        )
        latency_ms = (time.perf_counter() - t0) * 1000

        # Extract logprobs of tokens "0" and "1" and apply softmax.
        logprob_0: float | None = None
        logprob_1: float | None = None

        logprobs_list = body.get("logprobs")
        if isinstance(logprobs_list, list) and logprobs_list:
            first_entry = logprobs_list[0]
            if isinstance(first_entry, dict):
                top_lps = first_entry.get("top_logprobs") or []
                for entry in top_lps:
                    if isinstance(entry, dict):
                        # Strip the token so that " 0" (leading-space variant
                        # from some tokenizers) still matches "0"/"1".
                        token = (entry.get("token") or "").strip()
                        if token == "0":
                            logprob_0 = entry.get("logprob")
                        elif token == "1":
                            logprob_1 = entry.get("logprob")

        # Extract the generated text once — used by both the fallback
        # probability path and the return dict.
        raw_text = body.get("response", "").strip()

        def _fallback_p(raw: str) -> float:
            """Text-based fallback probability when logprobs are unusable."""
            raw_token = raw[:1] if raw else ""
            if raw_token == "0":
                return 0.95
            elif raw_token == "1":
                return 0.05
            log.warning(
                "No usable logprobs for 0/1 and unexpected token: %r",
                raw[:100],
            )
            return 0.5

        low_confidence = False
        if logprob_0 is not None and logprob_1 is not None:
            # Guard: the model assigns probability across the entire
            # vocabulary.  When P("0")+P("1") is low the 2-token softmax
            # renormalises residual mass the model actually spread over
            # other tokens — p_harmful becomes noise.  Degrade to the
            # text-based fallback and flag the result.
            total_prob = math.exp(logprob_0) + math.exp(logprob_1)
            if total_prob < _MIN_TOTAL_PROB:
                log.warning(
                    "Low total prob mass on 0/1 tokens: %.4f (model=%s); "
                    "degrading to fallback",
                    total_prob,
                    self._model,
                )
                low_confidence = True
                p_harmful = _fallback_p(raw_text)
            else:
                max_lp = max(logprob_0, logprob_1)
                exp_0 = math.exp(logprob_0 - max_lp)
                exp_1 = math.exp(logprob_1 - max_lp)
                p_harmful = exp_0 / (exp_0 + exp_1)
        else:
            # Fallback: parse generated text when logprobs are unavailable.
            low_confidence = True
            p_harmful = _fallback_p(raw_text)

        verdict = "block" if p_harmful > self._harmful_threshold else "pass"

        return {
            "verdict": verdict,
            "raw_token": raw_text[:1] if raw_text else "",
            "raw_text": raw_text,
            "p_harmful": round(p_harmful, 4),
            "low_confidence": low_confidence,
            "history_turns": history_turns,
            "latency_ms": round(latency_ms, 2),
        }

    # ------------------------------------------------------------------

    @property
    def model_name(self) -> str:
        return self._model
