"""Unit tests for security_middleware.invoke — orchestration entry point."""

import unittest
from unittest.mock import MagicMock, patch

from agent_sec_cli.correlation_context import (
    TraceContext,
    clear_process_trace_context,
    get_current_trace_context,
    init_process_trace_context,
)
from agent_sec_cli.security_middleware import (
    _detect_caller,
    invoke,
    invoke_with_context,
)
from agent_sec_cli.security_middleware.result import ActionResult


class TestDetectCaller(unittest.TestCase):
    def test_returns_unknown_in_test_context(self):
        caller = _detect_caller()
        self.assertEqual(caller, "unknown")


class TestInvoke(unittest.TestCase):
    def tearDown(self):
        clear_process_trace_context()

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
    def test_invoke_with_context_uses_explicit_caller_and_trace_context(
        self,
        mock_pre,
        mock_post,
        mock_get_backend,
    ):
        mock_backend = MagicMock()
        mock_backend.execute.return_value = ActionResult(success=True)
        mock_get_backend.return_value = mock_backend

        result = invoke_with_context(
            "prompt_scan",
            caller="daemon",
            trace_context={
                "traceId": "trace-1",
                "session_id": "session-1",
                "runId": "run-1",
                "call_id": "call-1",
                "toolCallId": "tool-1",
            },
            text="hello",
        )

        self.assertTrue(result.success)
        ctx = mock_backend.execute.call_args.args[0]
        self.assertEqual(ctx.action, "prompt_scan")
        self.assertEqual(ctx.caller, "daemon")
        self.assertEqual(ctx.trace_id, "trace-1")
        self.assertEqual(ctx.session_id, "session-1")
        self.assertEqual(ctx.run_id, "run-1")
        self.assertEqual(ctx.call_id, "call-1")
        self.assertEqual(ctx.tool_call_id, "tool-1")
        mock_pre.assert_called_once()
        mock_post.assert_called_once()

    @patch("agent_sec_cli.security_middleware.router.get_backend")
    def test_invoke_with_context_suppresses_process_trace_context(
        self,
        mock_get_backend,
    ):
        init_process_trace_context(TraceContext(session_id="process-session"))
        mock_backend = MagicMock()
        mock_backend.execute.return_value = ActionResult(success=True)
        mock_get_backend.return_value = mock_backend

        invoke_with_context("prompt_scan", caller="daemon", text="hello")

        ctx = mock_backend.execute.call_args.args[0]
        self.assertIsNone(ctx.session_id)
        self.assertEqual(
            get_current_trace_context(),
            TraceContext(session_id="process-session"),
        )


if __name__ == "__main__":
    unittest.main()
