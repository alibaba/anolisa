"""Unit tests for security_middleware.backends.hardening — HardeningBackend.

All subprocess calls are mocked. Tests focus on:
  - Command construction (_build_command)
  - ANSI stripping
  - Summary line parsing (all 6 counters)
  - Per-rule failure extraction
  - Engine Error detection
  - FileNotFoundError handling
"""

import subprocess
import unittest
from unittest.mock import patch

from agent_sec_cli.security_middleware.backends.hardening import (
    HardeningBackend,
    _strip_ansi,
)
from agent_sec_cli.security_middleware.context import RequestContext

# ---------------------------------------------------------------------------
# Realistic loongshield output fixtures
# ---------------------------------------------------------------------------

LOONGSHIELD_ALL_PASS = """\
\x1b[32m[INFO  10:07:54]\x1b[0m engine.lua:150: [1.1.1] PASS: Ensure mounting of cramfs is disabled
\x1b[32m[INFO  10:07:54]\x1b[0m engine.lua:150: [1.1.2] PASS: Ensure mounting of squashfs is disabled
\x1b[32m[INFO  10:08:01]\x1b[0m engine.lua:292: SEHarden Finished. 23 passed, 0 fixed, 0 failed, 0 manual, 0 dry-run-pending / 23 total.
"""

LOONGSHIELD_WITH_FAILURES = """\
\x1b[32m[INFO  14:30:00]\x1b[0m engine.lua:150: [1.1.1] PASS: Ensure cramfs disabled
\x1b[33m[WARN  14:30:01]\x1b[0m engine.lua:186: [fs.udf_disabled] FAIL: Ensure mounting of udf is disabled
\x1b[33m[WARN  14:30:02]\x1b[0m engine.lua:186: [time.sync_enabled] FAIL: Ensure time sync is enabled
\x1b[32m[INFO  14:30:03]\x1b[0m engine.lua:292: [audit.5.1.1] MANUAL: No reinforce steps for audit rules
\x1b[32m[INFO  14:30:04]\x1b[0m engine.lua:292: SEHarden Finished. 20 passed, 0 fixed, 2 failed, 1 manual, 0 dry-run-pending / 23 total.
"""

LOONGSHIELD_REINFORCE = """\
\x1b[31m[ERROR 14:30:04]\x1b[0m engine.lua:307: [fs.shadow_perms] FAILED-TO-FIX: Cannot set file permissions on /etc/shadow
\x1b[31m[ERROR 14:30:04]\x1b[0m engine.lua:295: [kern.sysctl_apply] ENFORCE-ERROR: Failed to apply sysctl setting
\x1b[32m[INFO  14:30:05]\x1b[0m engine.lua:292: SEHarden Finished. 18 passed, 3 fixed, 1 failed, 0 manual, 0 dry-run-pending / 22 total.
"""

LOONGSHIELD_DRYRUN = """\
\x1b[32m[INFO  14:30:01]\x1b[0m engine.lua:298: [fs.cramfs_blacklist] DRY-RUN: would apply cramfs blacklist
\x1b[32m[INFO  14:30:02]\x1b[0m engine.lua:298: [svc.chronyd_enable] DRY-RUN: would enable chronyd
\x1b[32m[INFO  14:30:03]\x1b[0m engine.lua:292: SEHarden Finished. 20 passed, 0 fixed, 0 failed, 0 manual, 2 dry-run-pending / 22 total.
"""

LOONGSHIELD_ENGINE_ERROR = """\
\x1b[31m[ERROR 14:30:04]\x1b[0m engine.lua:350: Engine Error: config file not found: /etc/missing.conf
\x1b[32m[INFO  14:30:05]\x1b[0m engine.lua:292: SEHarden Finished. 0 passed, 0 fixed, 0 failed, 0 manual, 0 dry-run-pending / 0 total.
"""


def _mock_proc(stdout, returncode=0):
    """Create a mock subprocess.CompletedProcess."""
    return subprocess.CompletedProcess(
        args=["loongshield", "seharden"], returncode=returncode, stdout=stdout
    )


class TestBuildCommand(unittest.TestCase):
    def test_scan_mode(self):
        cmd = HardeningBackend._build_command("scan", "agentos_baseline")
        self.assertEqual(cmd, ["loongshield", "seharden", "--scan", "--config", "agentos_baseline"])

    def test_reinforce_mode(self):
        cmd = HardeningBackend._build_command("reinforce", "agentos_baseline")
        self.assertEqual(cmd, ["loongshield", "seharden", "--reinforce", "--config", "agentos_baseline"])

    def test_dryrun_mode(self):
        cmd = HardeningBackend._build_command("dry-run", "agentos_baseline")
        self.assertEqual(cmd, ["loongshield", "seharden", "--reinforce", "--dry-run", "--config", "agentos_baseline"])

    def test_custom_config(self):
        cmd = HardeningBackend._build_command("scan", "custom_cfg")
        self.assertIn("custom_cfg", cmd)


class TestStripAnsi(unittest.TestCase):
    def test_strips_colour_codes(self):
        raw = "\x1b[32mGREEN\x1b[0m normal"
        self.assertEqual(_strip_ansi(raw), "GREEN normal")

    def test_no_ansi_unchanged(self):
        self.assertEqual(_strip_ansi("plain text"), "plain text")


class TestHardeningExecute(unittest.TestCase):
    def setUp(self):
        self.backend = HardeningBackend()
        self.ctx = RequestContext(action="harden")

    @patch("subprocess.run")
    def test_all_pass(self, mock_run):
        mock_run.return_value = _mock_proc(LOONGSHIELD_ALL_PASS, 0)
        result = self.backend.execute(self.ctx, mode="scan")

        self.assertTrue(result.success)
        self.assertEqual(result.data["passed"], 23)
        self.assertEqual(result.data["failed"], 0)
        self.assertEqual(result.data["total"], 23)
        self.assertEqual(result.data["failures"], [])
        self.assertEqual(result.data["fixed_items"], [])

    @patch("subprocess.run")
    def test_with_failures(self, mock_run):
        mock_run.return_value = _mock_proc(LOONGSHIELD_WITH_FAILURES, 1)
        result = self.backend.execute(self.ctx, mode="scan")

        self.assertFalse(result.success)
        self.assertEqual(result.data["passed"], 20)
        self.assertEqual(result.data["failed"], 2)
        self.assertEqual(result.data["manual"], 1)
        # In scan mode, all non-PASS go to failures
        self.assertEqual(len(result.data["failures"]), 3)
        self.assertEqual(result.data["fixed_items"], [])

        statuses = [f["status"] for f in result.data["failures"]]
        self.assertIn("FAIL", statuses)
        self.assertIn("MANUAL", statuses)

    @patch("subprocess.run")
    def test_reinforce_failures(self, mock_run):
        mock_run.return_value = _mock_proc(LOONGSHIELD_REINFORCE, 1)
        result = self.backend.execute(self.ctx, mode="reinforce")

        self.assertEqual(result.data["fixed"], 3)
        # FAILED-TO-FIX / ENFORCE-ERROR → failures (unresolved)
        statuses = [f["status"] for f in result.data["failures"]]
        self.assertIn("FAILED-TO-FIX", statuses)
        self.assertIn("ENFORCE-ERROR", statuses)
        # No plain FAIL lines in this fixture → fixed_items empty
        self.assertEqual(result.data["fixed_items"], [])

    @patch("subprocess.run")
    def test_dryrun_entries(self, mock_run):
        mock_run.return_value = _mock_proc(LOONGSHIELD_DRYRUN, 0)
        result = self.backend.execute(self.ctx, mode="dry-run")

        self.assertTrue(result.success)
        self.assertEqual(result.data["dry_run_pending"], 2)
        # dry-run is not reinforce, so all entries go to failures
        statuses = [f["status"] for f in result.data["failures"]]
        self.assertIn("DRY-RUN", statuses)

    @patch("subprocess.run")
    def test_engine_error(self, mock_run):
        mock_run.return_value = _mock_proc(LOONGSHIELD_ENGINE_ERROR, 1)
        result = self.backend.execute(self.ctx, mode="scan")

        engine_errors = [
            f for f in result.data["failures"] if f["status"] == "Engine Error"
        ]
        self.assertEqual(len(engine_errors), 1)
        self.assertIn("config file not found", engine_errors[0]["message"])

    @patch("subprocess.run")
    def test_file_not_found(self, mock_run):
        mock_run.side_effect = FileNotFoundError("loongshield not found")
        result = self.backend.execute(self.ctx, mode="scan")

        self.assertFalse(result.success)
        self.assertEqual(result.exit_code, 127)
        self.assertIn("command not found", result.error)

    @patch("subprocess.run")
    def test_no_summary_line(self, mock_run):
        mock_run.return_value = _mock_proc("some random output\n", 0)
        result = self.backend.execute(self.ctx, mode="scan")

        # No summary parsed → no counter keys
        self.assertNotIn("passed", result.data)
        self.assertTrue(result.success)

    @patch("subprocess.run")
    def test_alphanumeric_rule_ids(self, mock_run):
        """Rule IDs like fs.shm_noexec must be captured."""
        output = (
            "\x1b[33m[WARN  12:28:06]\x1b[0m engine.lua:186: "
            "[fs.shm_noexec] FAIL: /dev/shm must be mounted noexec\n"
            "\x1b[32m[INFO  12:28:06]\x1b[0m engine.lua:316: "
            "SEHarden Finished. 22 passed, 0 fixed, 1 failed, 0 manual, "
            "0 dry-run-pending / 23 total.\n"
        )
        mock_run.return_value = _mock_proc(output, 1)
        result = self.backend.execute(self.ctx, mode="scan")

        self.assertEqual(len(result.data["failures"]), 1)
        self.assertEqual(result.data["failures"][0]["rule_id"], "fs.shm_noexec")
        self.assertEqual(result.data["failures"][0]["status"], "FAIL")
        self.assertIn("noexec", result.data["failures"][0]["message"])

    @patch("subprocess.run")
    def test_fallback_when_failures_unparsed(self, mock_run):
        """If summary reports failures but regex misses them, add UNKNOWN entry."""
        output = (
            "[WARN  14:30:01] engine.lua:186: [~~weird~~] ???: something odd\n"
            "[INFO  14:30:02] engine.lua:292: SEHarden Finished. "
            "22 passed, 0 fixed, 1 failed, 0 manual, 0 dry-run-pending / 23 total.\n"
        )
        mock_run.return_value = _mock_proc(output, 1)
        result = self.backend.execute(self.ctx, mode="scan")

        self.assertEqual(len(result.data["failures"]), 1)
        self.assertEqual(result.data["failures"][0]["status"], "UNKNOWN")
        self.assertIn("could not be parsed", result.data["failures"][0]["message"])

    @patch("subprocess.run")
    def test_reinforce_fail_goes_to_fixed_items(self, mock_run):
        """In reinforce mode, FAIL entries are remediated → fixed_items."""
        output = (
            "\x1b[33m[WARN  12:28:06]\x1b[0m engine.lua:186: "
            "[fs.shm_noexec] FAIL: /dev/shm must be mounted noexec\n"
            "\x1b[32m[INFO  12:28:06]\x1b[0m engine.lua:316: "
            "SEHarden Finished. 22 passed, 1 fixed, 0 failed, 0 manual, "
            "0 dry-run-pending / 23 total.\n"
        )
        mock_run.return_value = _mock_proc(output, 0)
        result = self.backend.execute(self.ctx, mode="reinforce")

        self.assertEqual(result.data["failures"], [])
        self.assertEqual(len(result.data["fixed_items"]), 1)
        self.assertEqual(result.data["fixed_items"][0]["rule_id"], "fs.shm_noexec")

    @patch("subprocess.run")
    def test_ansi_stripped_in_stdout(self, mock_run):
        mock_run.return_value = _mock_proc(LOONGSHIELD_ALL_PASS, 0)
        result = self.backend.execute(self.ctx, mode="scan")

        # stdout must be ANSI-free
        self.assertNotIn("\x1b[", result.stdout)


if __name__ == "__main__":
    unittest.main()
