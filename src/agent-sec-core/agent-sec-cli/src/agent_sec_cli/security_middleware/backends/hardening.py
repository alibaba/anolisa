"""Hardening backend — subprocess wrapper for loongshield SEHarden.

Output format (verified against loongshield source):

    Log lines:  ``[LEVEL HH:MM:SS] source.lua:N: message``
    Summary:    ``SEHarden Finished. X passed, Y fixed, Z failed, W manual, V dry-run-pending / N total.``
    Per-rule:   ``[rule_id] STATUS: description``

ANSI colour codes are stripped before parsing.
"""

from __future__ import annotations

import re
import subprocess
from agent_sec_cli.security_middleware.result import ActionResult

# ---------------------------------------------------------------------------
# ANSI escape sequence stripper
# ---------------------------------------------------------------------------
_ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")


def _strip_ansi(text: str) -> str:
    """Remove ANSI colour / style escape sequences from *text*."""
    return _ANSI_RE.sub("", text)


# ---------------------------------------------------------------------------
# Per-rule status patterns  (applied to ANSI-stripped lines)
#
# Matches lines like:
#   [WARN  14:30:01] engine.lua:186: [1.1.3] FAIL: Ensure mounting of udf …
#   [ERROR 14:30:04] engine.lua:307: [3.2.1] FAILED-TO-FIX: …
#   [ERROR 14:30:04] engine.lua:295: [6.1.1] ENFORCE-ERROR: …
#   [INFO  14:30:01] engine.lua:298: [1.1.3] DRY-RUN: would apply …
#   [INFO  14:30:04] engine.lua:292: [5.1.1] MANUAL: No reinforce steps …
#   [ERROR …] engine.lua:…: Engine Error: …
# ---------------------------------------------------------------------------
_RULE_STATUS_RE = re.compile(
    r"\[(?P<rule_id>[\w.]+)\]\s+"
    r"(?P<status>FAIL|FAILED|FAILED-TO-FIX|ERROR|ENFORCE-ERROR|DRY-RUN|MANUAL|SKIP):\s*"
    r"(?P<message>.+?)\s*$"
)

_ENGINE_ERROR_RE = re.compile(
    r"Engine\s+Error:\s*(?P<message>.+?)\s*$"
)


class HardeningBackend:
    """Run ``loongshield seharden`` and parse its summary output."""

    # Summary line — captures all 6 counters.
    # Example: SEHarden Finished. 42 passed, 0 fixed, 0 failed, 0 manual, 0 dry-run-pending / 42 total.
    _SUMMARY_RE = re.compile(
        r"SEHarden\s+Finished\.\s*"
        r"(?P<passed>\d+)\s+passed,\s*"
        r"(?P<fixed>\d+)\s+fixed,\s*"
        r"(?P<failed>\d+)\s+failed,\s*"
        r"(?P<manual>\d+)\s+manual,\s*"
        r"(?P<dry_run_pending>\d+)\s+dry-run-pending\s*/\s*"
        r"(?P<total>\d+)\s+total\."
    )

    def execute(
        self,
        ctx,
        mode: str = "scan",
        config: str = "agentos_baseline",
        **kwargs,
    ) -> ActionResult:
        """Execute loongshield seharden in the requested *mode*.

        Modes:
            scan      — ``--scan``
            reinforce — ``--reinforce``
            dry-run   — ``--reinforce --dry-run``
        """
        cmd = self._build_command(mode, config)

        try:
            proc = subprocess.run(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            )
        except FileNotFoundError:
            return ActionResult(
                success=False,
                exit_code=127,
                error="loongshield: command not found",
            )

        raw_output = proc.stdout or ""
        clean_output = _strip_ansi(raw_output)

        data: dict = {"mode": mode, "config": config, "failures": [], "fixed_items": []}

        # --- Parse summary line ---
        for line in reversed(clean_output.splitlines()):
            m = self._SUMMARY_RE.search(line)
            if m:
                data["passed"] = int(m.group("passed"))
                data["fixed"] = int(m.group("fixed"))
                data["failed"] = int(m.group("failed"))
                data["manual"] = int(m.group("manual"))
                data["dry_run_pending"] = int(m.group("dry_run_pending"))
                data["total"] = int(m.group("total"))
                break

        # --- Collect per-rule non-PASS entries ---
        entries: list[dict] = []
        for line in clean_output.splitlines():
            m = _RULE_STATUS_RE.search(line)
            if m:
                entries.append({
                    "rule_id": m.group("rule_id"),
                    "status": m.group("status"),
                    "message": m.group("message").strip(),
                })
                continue
            m = _ENGINE_ERROR_RE.search(line)
            if m:
                entries.append({
                    "rule_id": "",
                    "status": "Engine Error",
                    "message": m.group("message").strip(),
                })

        # --- Partition into failures vs fixed_items ---
        # In reinforce mode, FAIL/FAILED lines are pre-fix detections;
        # loongshield emits FAILED-TO-FIX / ENFORCE-ERROR for rules it
        # could *not* fix, so plain FAIL/FAILED entries were remediated.
        _FIX_DETECTED = frozenset({"FAIL", "FAILED"})
        if mode == "reinforce":
            data["failures"] = [e for e in entries if e["status"] not in _FIX_DETECTED]
            data["fixed_items"] = [e for e in entries if e["status"] in _FIX_DETECTED]
        else:
            data["failures"] = entries

        # Fallback: if summary reports non-pass rules but nothing was captured,
        # add a warning so the event is never silently incomplete.
        reported_nonpass = (
            data.get("failed", 0) + data.get("manual", 0)
            + data.get("fixed", 0) + data.get("dry_run_pending", 0)
        )
        if reported_nonpass > 0 and not data["failures"] and not data["fixed_items"]:
            data["failures"].append({
                "rule_id": "",
                "status": "UNKNOWN",
                "message": (
                    f"Summary reports {reported_nonpass} non-pass rule(s) "
                    "but per-rule details could not be parsed from output."
                ),
            })

        return ActionResult(
            success=(proc.returncode == 0),
            stdout=clean_output,
            exit_code=proc.returncode,
            data=data,
        )

    # ------------------------------------------------------------------
    @staticmethod
    def _build_command(mode: str, config: str) -> list[str]:
        cmd = ["loongshield", "seharden"]
        if mode == "dry-run":
            cmd += ["--reinforce", "--dry-run"]
        elif mode == "reinforce":
            cmd.append("--reinforce")
        else:
            cmd.append("--scan")
        cmd += ["--config", config]
        return cmd
