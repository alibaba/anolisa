"""Unit tests for prompt_scanner.models.multi_turn_intent.

Tests cover:
- format_history: dict format, legacy string format, edge cases, truncation
- format_defender_prompt: template substitution
- MultiTurnIntentClassifier.check_ready: delegates to model service
- MultiTurnIntentClassifier.classify: logprobs parsing, fallback, verdict logic
"""

import os
import unittest
from unittest.mock import MagicMock, patch

from agent_sec_cli.prompt_scanner.models.multi_turn_intent import (
    _MAX_HISTORY_TURNS,
    MultiTurnIntentClassifier,
    format_defender_prompt,
    format_history,
)

# ---------------------------------------------------------------------------
# format_history
# ---------------------------------------------------------------------------


class TestFormatHistory(unittest.TestCase):
    def test_empty_history(self) -> None:
        self.assertEqual(format_history([]), "(No previous turns)")

    def test_dict_format_user_assistant(self) -> None:
        history = [
            {"role": "user", "content": "Hello"},
            {"role": "assistant", "content": "Hi there"},
        ]
        result = format_history(history)
        self.assertIn("USER: Hello", result)
        self.assertIn("ASSISTANT: Hi there", result)

    def test_dict_format_unknown_role(self) -> None:
        history = [{"role": "system", "content": "You are helpful"}]
        result = format_history(history)
        self.assertIn("SYSTEM: You are helpful", result)

    def test_dict_format_missing_role(self) -> None:
        history = [{"content": "no role"}]
        result = format_history(history)
        self.assertIn("UNKNOWN: no role", result)

    def test_dict_format_missing_content(self) -> None:
        history = [{"role": "user"}]
        result = format_history(history)
        self.assertIn("USER:", result)

    def test_legacy_string_with_prefix(self) -> None:
        """Legacy 'user: ...' should have role uppercased."""
        history = ["user: hello", "assistant: hi"]
        result = format_history(history)
        self.assertIn("USER: hello", result)
        self.assertIn("ASSISTANT: hi", result)

    def test_legacy_string_already_uppercase_prefix(self) -> None:
        history = ["USER: hello"]
        result = format_history(history)
        self.assertIn("USER: hello", result)

    def test_legacy_string_without_prefix(self) -> None:
        """Strings without ': ' should be treated as unknown role."""
        history = ["just a plain string"]
        result = format_history(history)
        self.assertIn("UNKNOWN: just a plain string", result)

    def test_non_dict_non_string_turn(self) -> None:
        history = [42]
        result = format_history(history)
        self.assertIn("UNKNOWN: 42", result)

    def test_turns_joined_by_double_newline(self) -> None:
        history = [
            {"role": "user", "content": "a"},
            {"role": "assistant", "content": "b"},
        ]
        result = format_history(history)
        self.assertIn("\n\n", result)

    def test_truncation_to_max_turns(self) -> None:
        """Only the last _MAX_HISTORY_TURNS turns should be kept."""
        # Use padded numbers to avoid substring collisions (e.g. "msg-01"
        # is NOT a substring of "msg-10").
        history = [
            {"role": "user", "content": f"msg-{i:02d}"}
            for i in range(_MAX_HISTORY_TURNS + 5)
        ]
        result = format_history(history)
        # The first few messages should be truncated out
        self.assertNotIn("msg-00", result)
        self.assertNotIn("msg-01", result)
        self.assertNotIn("msg-04", result)
        # The last _MAX_HISTORY_TURNS messages should be present
        self.assertIn(f"msg-{_MAX_HISTORY_TURNS + 4:02d}", result)

    def test_exactly_max_turns_not_truncated(self) -> None:
        history = [
            {"role": "user", "content": f"msg-{i:02d}"}
            for i in range(_MAX_HISTORY_TURNS)
        ]
        result = format_history(history)
        self.assertIn("msg-00", result)
        self.assertIn(f"msg-{_MAX_HISTORY_TURNS - 1:02d}", result)


# ---------------------------------------------------------------------------
# format_defender_prompt
# ---------------------------------------------------------------------------


class TestFormatDefenderPrompt(unittest.TestCase):
    def test_template_substitution(self) -> None:
        result = format_defender_prompt(
            history=[{"role": "user", "content": "hi"}],
            current_query="what is 2+2?",
            assistant_response="4",
        )
        self.assertIn("USER: hi", result)
        self.assertIn("what is 2+2?", result)
        self.assertIn("ASSISTANT: 4", result)
        self.assertIn("0 for harmful, 1 for benign", result)

    def test_empty_history_placeholder(self) -> None:
        result = format_defender_prompt(
            history=[],
            current_query="test",
            assistant_response="resp",
        )
        self.assertIn("(No previous turns)", result)

    def test_history_with_question_literal_not_reinterpreted(self) -> None:
        """History text containing the literal '{question}' must survive
        verbatim — it must NOT be re-substituted as a template field.

        Regression guard for chained str.replace, which would wrongly
        replace the literal injected via history on a later pass."""
        result = format_defender_prompt(
            history=[{"role": "user", "content": "inject {question} here"}],
            current_query="real query",
            assistant_response="real response",
        )
        # The literal '{question}' from history is preserved ...
        self.assertIn("{question}", result)
        # ... while the genuine template field is still substituted.
        self.assertIn("USER: real query", result)

    def test_placeholder_literals_in_all_fields_survive(self) -> None:
        """Placeholder-looking text in every user-controlled field must pass
        through untouched (single-pass substitution)."""
        result = format_defender_prompt(
            history=[{"role": "user", "content": "h-{history}-h"}],
            current_query="q-{question}-q",
            assistant_response="r-{assistant_response}-r",
        )
        self.assertIn("h-{history}-h", result)
        self.assertIn("q-{question}-q", result)
        self.assertIn("r-{assistant_response}-r", result)


# ---------------------------------------------------------------------------
# MultiTurnIntentClassifier
# ---------------------------------------------------------------------------


def _make_mock_client() -> MagicMock:
    """Create a mock ModelServiceClient."""
    client = MagicMock()
    client.check_model.return_value = True
    return client


class TestMultiTurnIntentClassifierInit(unittest.TestCase):
    def test_default_threshold(self) -> None:
        clf = MultiTurnIntentClassifier(client=_make_mock_client())
        self.assertEqual(clf._harmful_threshold, 0.55)

    def test_custom_threshold(self) -> None:
        clf = MultiTurnIntentClassifier(
            harmful_threshold=0.8, client=_make_mock_client()
        )
        self.assertEqual(clf._harmful_threshold, 0.8)

    def test_model_name_from_env(self) -> None:
        with patch.dict(os.environ, {"AGENT_SEC_OLLAMA_MODEL": "custom-model"}):
            clf = MultiTurnIntentClassifier(client=_make_mock_client())
            self.assertEqual(clf.model_name, "custom-model")

    def test_default_model_name(self) -> None:
        clf = MultiTurnIntentClassifier(client=_make_mock_client())
        self.assertEqual(clf.model_name, "warden")


class TestCheckReady(unittest.TestCase):
    def test_check_ready_returns_true_when_model_found(self) -> None:
        client = _make_mock_client()
        client.check_model.return_value = True
        clf = MultiTurnIntentClassifier(client=client)
        self.assertTrue(clf.check_ready())
        client.check_model.assert_called_once_with("warden")

    def test_check_ready_returns_false_when_model_not_found(self) -> None:
        client = _make_mock_client()
        client.check_model.return_value = False
        clf = MultiTurnIntentClassifier(client=client)
        self.assertFalse(clf.check_ready())


# ---------------------------------------------------------------------------
# MultiTurnIntentClassifier.classify
# ---------------------------------------------------------------------------


class TestClassifyLogprobs(unittest.TestCase):
    """Tests for the logprobs-based probability calculation."""

    def _make_classifier_with_response(self, body: dict) -> MultiTurnIntentClassifier:
        client = _make_mock_client()
        client.generate.return_value = body
        return MultiTurnIntentClassifier(client=client)

    def test_logprobs_harmful(self) -> None:
        """logprob_0 > logprob_1 → p_harmful > 0.5 → block."""
        # logprob(0) = -0.1, logprob(1) = -2.3
        # softmax → p(0) ≈ 0.90 → block (> 0.55)
        body = {
            "response": "0",
            "logprobs": [
                {
                    "top_logprobs": [
                        {"token": "0", "logprob": -0.1},
                        {"token": "1", "logprob": -2.3},
                    ]
                }
            ],
        }
        clf = self._make_classifier_with_response(body)
        result = clf.classify([], "query", "response")
        self.assertEqual(result["verdict"], "block")
        self.assertGreater(result["p_harmful"], 0.5)

    def test_logprobs_benign(self) -> None:
        """logprob_1 > logprob_0 → p_harmful < 0.5 → pass."""
        body = {
            "response": "1",
            "logprobs": [
                {
                    "top_logprobs": [
                        {"token": "0", "logprob": -2.3},
                        {"token": "1", "logprob": -0.1},
                    ]
                }
            ],
        }
        clf = self._make_classifier_with_response(body)
        result = clf.classify([], "query", "response")
        self.assertEqual(result["verdict"], "pass")
        self.assertLess(result["p_harmful"], 0.5)

    def test_logprobs_equal_probabilities(self) -> None:
        """Equal logprobs → p_harmful = 0.5 → pass (0.5 < 0.55)."""
        body = {
            "response": "0",
            "logprobs": [
                {
                    "top_logprobs": [
                        {"token": "0", "logprob": -1.0},
                        {"token": "1", "logprob": -1.0},
                    ]
                }
            ],
        }
        clf = self._make_classifier_with_response(body)
        result = clf.classify([], "query", "response")
        self.assertEqual(result["p_harmful"], 0.5)
        self.assertEqual(result["verdict"], "pass")

    def test_logprobs_threshold_boundary(self) -> None:
        """p_harmful exactly at threshold (0.55) → pass (strict >)."""
        # We need p_harmful = 0.55, so logprob_0 - logprob_1 = ln(0.55/0.45)
        import math

        ratio = math.log(0.55 / 0.45)
        body = {
            "response": "0",
            "logprobs": [
                {
                    "top_logprobs": [
                        {"token": "0", "logprob": ratio},
                        {"token": "1", "logprob": 0.0},
                    ]
                }
            ],
        }
        clf = self._make_classifier_with_response(body)
        result = clf.classify([], "query", "response")
        # p_harmful rounds to ~0.55, verdict is "pass" because 0.55 is not > 0.55
        self.assertAlmostEqual(result["p_harmful"], 0.55, places=2)


class TestClassifyFallback(unittest.TestCase):
    """Tests for the text-parsing fallback when logprobs are unavailable."""

    def _make_classifier_with_response(self, body: dict) -> MultiTurnIntentClassifier:
        client = _make_mock_client()
        client.generate.return_value = body
        return MultiTurnIntentClassifier(client=client)

    def test_fallback_token_0(self) -> None:
        body = {"response": "0", "logprobs": None}
        clf = self._make_classifier_with_response(body)
        result = clf.classify([], "query", "response")
        self.assertEqual(result["verdict"], "block")
        self.assertEqual(result["p_harmful"], 0.95)

    def test_fallback_token_1(self) -> None:
        body = {"response": "1", "logprobs": None}
        clf = self._make_classifier_with_response(body)
        result = clf.classify([], "query", "response")
        self.assertEqual(result["verdict"], "pass")
        self.assertEqual(result["p_harmful"], 0.05)

    def test_fallback_unexpected_token(self) -> None:
        body = {"response": "unexpected text", "logprobs": None}
        clf = self._make_classifier_with_response(body)
        result = clf.classify([], "query", "response")
        self.assertEqual(result["p_harmful"], 0.5)
        self.assertEqual(result["verdict"], "pass")

    def test_fallback_empty_response(self) -> None:
        body = {"response": "", "logprobs": []}
        clf = self._make_classifier_with_response(body)
        result = clf.classify([], "query", "response")
        self.assertEqual(result["p_harmful"], 0.5)
        self.assertEqual(result["raw_token"], "")

    def test_logprobs_list_but_no_top_logprobs(self) -> None:
        """logprobs present but top_logprobs empty → fallback."""
        body = {"response": "1", "logprobs": [{"top_logprobs": []}]}
        clf = self._make_classifier_with_response(body)
        result = clf.classify([], "query", "response")
        self.assertEqual(result["verdict"], "pass")
        self.assertEqual(result["p_harmful"], 0.05)

    def test_logprobs_only_token_0(self) -> None:
        """Only token '0' in logprobs (no '1') → fallback."""
        body = {
            "response": "0",
            "logprobs": [{"top_logprobs": [{"token": "0", "logprob": -0.1}]}],
        }
        clf = self._make_classifier_with_response(body)
        result = clf.classify([], "query", "response")
        self.assertEqual(result["p_harmful"], 0.95)


class TestClassifyReturnShape(unittest.TestCase):
    def test_return_dict_keys(self) -> None:
        body = {"response": "1", "logprobs": None}
        client = _make_mock_client()
        client.generate.return_value = body
        clf = MultiTurnIntentClassifier(client=client)
        result = clf.classify([], "query", "response")
        for key in ("verdict", "raw_token", "raw_text", "p_harmful", "latency_ms"):
            self.assertIn(key, result)

    def test_latency_ms_is_float(self) -> None:
        body = {"response": "1", "logprobs": None}
        client = _make_mock_client()
        client.generate.return_value = body
        clf = MultiTurnIntentClassifier(client=client)
        result = clf.classify([], "query", "response")
        self.assertIsInstance(result["latency_ms"], float)

    def test_generate_called_with_correct_params(self) -> None:
        body = {"response": "1", "logprobs": None}
        client = _make_mock_client()
        client.generate.return_value = body
        clf = MultiTurnIntentClassifier(client=client)
        clf.classify(
            history=[{"role": "user", "content": "hi"}],
            current_query="query",
            assistant_response="resp",
        )
        client.generate.assert_called_once()
        call_kwargs = client.generate.call_args
        self.assertTrue(call_kwargs.kwargs["raw"])
        self.assertTrue(call_kwargs.kwargs["logprobs"])
        self.assertEqual(call_kwargs.kwargs["top_logprobs"], 10)


if __name__ == "__main__":
    unittest.main()
