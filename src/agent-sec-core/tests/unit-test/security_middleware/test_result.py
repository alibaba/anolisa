"""Unit tests for security_middleware.result — ActionResult dataclass."""

import unittest

from agent_sec_cli.security_middleware.result import ActionResult


class TestActionResult(unittest.TestCase):
    def test_defaults(self):
        r = ActionResult(success=True)
        self.assertTrue(r.success)
        self.assertEqual(r.data, {})
        self.assertEqual(r.stdout, "")
        self.assertEqual(r.exit_code, 0)
        self.assertEqual(r.error, "")

    def test_custom_values(self):
        r = ActionResult(
            success=False,
            data={"key": "val"},
            stdout="output",
            exit_code=42,
            error="boom",
        )
        self.assertFalse(r.success)
        self.assertEqual(r.data["key"], "val")
        self.assertEqual(r.stdout, "output")
        self.assertEqual(r.exit_code, 42)
        self.assertEqual(r.error, "boom")


if __name__ == "__main__":
    unittest.main()
