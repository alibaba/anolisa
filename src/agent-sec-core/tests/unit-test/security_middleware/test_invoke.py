"""Unit tests for security_middleware.invoke — orchestration entry point."""

import unittest
from unittest.mock import MagicMock, patch

from agent_sec_cli.security_middleware import _detect_caller, invoke
from agent_sec_cli.security_middleware.result import ActionResult


class TestDetectCaller(unittest.TestCase):
    def test_returns_unknown_in_test_context(self):
        caller = _detect_caller()
        self.assertEqual(caller, "unknown")


class TestInvoke(unittest.TestCase):
    @patch("agent_sec_cli.security_middleware.router.get_backend")
    @patch("agent_sec_cli.security_middleware.lifecycle.post_action")
    @patch("agent_sec_cli.security_middleware.lifecycle.pre_action")
    def test_invoke_calls_lifecycle_hooks(self, mock_pre, mock_post, mock_get_backend):
        mock_backend = MagicMock()
        mock_backend.execute.return_value = ActionResult(success=True)
        mock_get_backend.return_value = mock_backend

        result = invoke("sandbox_prehook", command="ls")

        mock_pre.assert_called_once()
        mock_post.assert_called_once()
        self.assertTrue(result.success)

    @patch("agent_sec_cli.security_middleware.router.get_backend")
    @patch("agent_sec_cli.security_middleware.lifecycle.on_error")
    @patch("agent_sec_cli.security_middleware.lifecycle.pre_action")
    def test_invoke_calls_on_error_and_reraises(
        self, mock_pre, mock_on_err, mock_get_backend
    ):
        mock_backend = MagicMock()
        mock_backend.execute.side_effect = RuntimeError("backend boom")
        mock_get_backend.return_value = mock_backend

        with self.assertRaises(RuntimeError):
            invoke("sandbox_prehook", command="ls")

        mock_on_err.assert_called_once()

    @patch("agent_sec_cli.security_middleware.router.get_backend")
    def test_invoke_passes_kwargs_to_backend(self, mock_get_backend):
        mock_backend = MagicMock()
        mock_backend.execute.return_value = ActionResult(success=True, data={"k": "v"})
        mock_get_backend.return_value = mock_backend

        invoke("sandbox_prehook", command="ls", cwd="/tmp")

        _, call_kwargs = mock_backend.execute.call_args
        self.assertEqual(call_kwargs["command"], "ls")
        self.assertEqual(call_kwargs["cwd"], "/tmp")

    def test_invoke_unknown_action_raises(self):
        with self.assertRaises(ValueError):
            invoke("totally_unknown_action")

    @patch("agent_sec_cli.security_middleware.router.get_backend")
    @patch("agent_sec_cli.security_middleware.lifecycle.post_action")
    @patch("agent_sec_cli.security_middleware.lifecycle.pre_action")
    def test_invoke_logs_nonzero_result_as_warning(
        self, mock_pre, mock_post, mock_get_backend
    ):
        mock_backend = MagicMock()
        mock_backend.execute.return_value = ActionResult(
            success=False,
            exit_code=7,
            error="blocked",
        )
        mock_get_backend.return_value = mock_backend

        with self.assertLogs(
            "agent_sec_cli.security_middleware", level="WARNING"
        ) as cm:
            invoke("code_scan", code="danger")

        self.assertEqual(len(cm.records), 1)
        record = cm.records[0]
        self.assertEqual(record.levelname, "WARNING")
        # Domain fields live under `data` in the new schema.
        self.assertEqual(record.data["action"], "code_scan")
        self.assertEqual(record.data["caller"], "unknown")
        self.assertEqual(record.data["exit_code"], 7)
        self.assertGreaterEqual(record.data["duration_ms"], 0)

    @patch("agent_sec_cli.security_middleware.router.get_backend")
    @patch("agent_sec_cli.security_middleware.lifecycle.on_error")
    @patch("agent_sec_cli.security_middleware.lifecycle.pre_action")
    def test_invoke_logs_backend_exception_as_error(
        self, mock_pre, mock_on_error, mock_get_backend
    ):
        mock_backend = MagicMock()
        mock_backend.execute.side_effect = RuntimeError("backend boom")
        mock_get_backend.return_value = mock_backend

        with self.assertLogs("agent_sec_cli.security_middleware", level="ERROR") as cm:
            with self.assertRaises(RuntimeError):
                invoke("code_scan", code="danger")

        self.assertEqual(len(cm.records), 1)
        record = cm.records[0]
        self.assertEqual(record.levelname, "ERROR")
        # `action`/`caller`/`duration_ms` go under `data`; `error_type` comes
        # from exc_info via the handler's exception envelope, not extra=.
        self.assertEqual(record.data["action"], "code_scan")
        self.assertIsNotNone(record.exc_info)
        self.assertIs(record.exc_info[0], RuntimeError)


if __name__ == "__main__":
    unittest.main()
