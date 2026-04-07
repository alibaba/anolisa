"""Unit tests for security_middleware.backends.summary — SummaryBackend (stub)."""

import unittest

from agent_sec_cli.security_middleware.backends.summary import SummaryBackend
from agent_sec_cli.security_middleware.context import RequestContext


class TestSummaryBackend(unittest.TestCase):
    def test_always_fails(self):
        backend = SummaryBackend()
        ctx = RequestContext(action="summary")
        result = backend.execute(ctx)

        self.assertFalse(result.success)
        self.assertIn("not yet implemented", result.error.lower())


if __name__ == "__main__":
    unittest.main()
