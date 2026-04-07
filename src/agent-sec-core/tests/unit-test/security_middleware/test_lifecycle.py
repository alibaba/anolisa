"""Unit tests for security_middleware.lifecycle — pre/post/error hooks."""

import unittest
from unittest.mock import MagicMock, patch

from agent_sec_cli.security_middleware.context import RequestContext
from agent_sec_cli.security_middleware.lifecycle import (
    _category_for,
    on_error,
    post_action,
    pre_action,
)
from agent_sec_cli.security_middleware.result import ActionResult


class TestCategoryMapping(unittest.TestCase):
    def test_harden_maps_to_hardening(self):
        self.assertEqual(_category_for("harden"), "hardening")

    def test_sandbox_prehook_maps_to_sandbox(self):
        self.assertEqual(_category_for("sandbox_prehook"), "sandbox")

    def test_verify_maps_to_asset_verify(self):
        self.assertEqual(_category_for("verify"), "asset_verify")

    def test_summary_maps_to_summary(self):
        self.assertEqual(_category_for("summary"), "summary")

    def test_unknown_action_falls_back_to_action_name(self):
        self.assertEqual(_category_for("custom_thing"), "custom_thing")


class TestPreAction(unittest.TestCase):
    @patch("agent_sec_cli.security_middleware.lifecycle.log_event")
    def test_pre_action_does_not_log(self, mock_log):
        ctx = RequestContext(action="harden")
        pre_action(ctx, {"mode": "scan"})
        mock_log.assert_not_called()


class TestPostAction(unittest.TestCase):
    @patch("agent_sec_cli.security_middleware.lifecycle.log_event")
    def test_post_action_logs_event(self, mock_log):
        ctx = RequestContext(action="harden", trace_id="t-123")
        result = ActionResult(success=True, data={"passed": 5})
        post_action(ctx, result, {"mode": "scan"})

        mock_log.assert_called_once()
        event = mock_log.call_args[0][0]
        self.assertEqual(event.event_type, "harden")
        self.assertEqual(event.category, "hardening")
        self.assertEqual(event.trace_id, "t-123")
        self.assertIn("request", event.details)
        self.assertIn("result", event.details)


class TestOnError(unittest.TestCase):
    @patch("agent_sec_cli.security_middleware.lifecycle.log_event")
    def test_on_error_logs_event(self, mock_log):
        ctx = RequestContext(action="verify", trace_id="t-456")
        exc = RuntimeError("test error")
        on_error(ctx, exc, {"skill": "/path"})

        mock_log.assert_called_once()
        event = mock_log.call_args[0][0]
        self.assertEqual(event.event_type, "verify_error")
        self.assertEqual(event.category, "asset_verify")
        self.assertIn("error", event.details)
        self.assertEqual(event.details["error"], "test error")
        self.assertEqual(event.details["error_type"], "RuntimeError")


if __name__ == "__main__":
    unittest.main()
