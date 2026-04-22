"""Hardening backend — passthrough wrapper for `loongshield seharden`.

The backend preserves the wrapper's legacy defaults and structured event data
while allowing callers to forward raw seharden arguments directly.
"""

import os
import re
import shutil
import subprocess
from typing import Any

from agent_sec_cli.security_middleware.backends.base import BaseBackend
from agent_sec_cli.security_middleware.context import RequestContext
from agent_sec_cli.security_middleware.result import ActionResult

DEFAULT_HARDEN_CONFIG = "agentos_baseline"
_DEFAULT_HARDEN_MODE = "scan"
_FALLBACK_LOONGSHIELD_PATHS = ("/usr/sbin/loongshield",)
_MISSING_LOONGSHIELD_ERROR = (
    "The `loongshield` command is required for `agent-sec-cli harden`, "
    "but it was not found.\n"
    "On ALinux 4 Operating System, you can usually install it from the default yum "
    "repository with:\n"
    "  sudo yum install -y loongshield\n"
    "If it is already installed, please make sure the `loongshield` binary is "
    "available in PATH."
)

_ANSI_RE = re.compile(r"\x1b\[[0-9;]*m")
_RULE_STATUS_RE = re.compile(
    r"\[(?P<rule_id>[\w.]+)\]\s+"
    r"(?P<status>FAIL|FAILED|FAILED-TO-FIX|ERROR|ENFORCE-ERROR|DRY-RUN|MANUAL|SKIP):\s*"
    r"(?P<message>.+?)\s*$"
)
_ENGINE_ERROR_RE = re.compile(r"Engine\s+Error:\s*(?P<message>.+?)\s*$")


def _strip_ansi(text: str) -> str:
    """Remove ANSI colour and style sequences from process output."""
    return _ANSI_RE.sub("", text)


class HardeningBackend(BaseBackend):
    """Execute `loongshield seharden` and keep structured hardening results."""

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
        ctx: RequestContext,
        args: list[str] | tuple[str, ...] | None = None,
        **kwargs: Any,
    ) -> ActionResult:
        """Execute `loongshield seharden` with raw args or legacy kwargs."""
        raw_args = self._normalize_args(args=args, **kwargs)
        mode, config = self._describe_request(raw_args)
        loongshield_path = self._resolve_loongshield_path()
        cmd = self._build_command(raw_args, loongshield_path=loongshield_path)
        data = self._build_result_data(
            raw_args=raw_args,
            cmd=cmd,
            tool_path=loongshield_path,
            mode=mode,
            config=config,
        )

        if not loongshield_path:
            return ActionResult(
                success=False,
                exit_code=127,
                error=_MISSING_LOONGSHIELD_ERROR,
                data=data,
            )

        try:
            proc = subprocess.run(
                cmd,
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            )
        except OSError as exc:
            return ActionResult(
                success=False,
                exit_code=getattr(exc, "errno", 1) or 1,
                error=f"Failed to execute `loongshield seharden`: {exc}",
                data=data,
            )

        clean_output = _strip_ansi(proc.stdout or "")
        data["returncode"] = proc.returncode
        self._parse_output(clean_output, data)

        return ActionResult(
            success=(proc.returncode == 0),
            stdout=clean_output,
            exit_code=proc.returncode,
            data=data,
        )

    @classmethod
    def _normalize_args(
        cls,
        args: list[str] | tuple[str, ...] | None = None,
        **kwargs: Any,
    ) -> list[str]:
        raw_args = [str(arg) for arg in (args or [])]
        if raw_args and kwargs:
            mixed_keys = ", ".join(sorted(kwargs))
            raise TypeError(
                f"Do not mix passthrough args with legacy harden kwargs: {mixed_keys}"
            )

        if raw_args:
            return raw_args

        legacy_keys = {"mode", "config"}
        unknown_keys = sorted(set(kwargs) - legacy_keys)
        if unknown_keys:
            unknown = ", ".join(unknown_keys)
            raise TypeError(f"Unsupported harden kwargs: {unknown}")

        if not kwargs:
            return cls._legacy_args(
                mode=_DEFAULT_HARDEN_MODE,
                config=DEFAULT_HARDEN_CONFIG,
            )

        mode = str(kwargs.get("mode", _DEFAULT_HARDEN_MODE))
        config = str(kwargs.get("config", DEFAULT_HARDEN_CONFIG))
        return cls._legacy_args(mode=mode, config=config)

    @staticmethod
    def _legacy_args(mode: str, config: str) -> list[str]:
        if mode == "dry-run":
            return ["--reinforce", "--dry-run", "--config", config]
        if mode == "reinforce":
            return ["--reinforce", "--config", config]
        if mode == "scan":
            return ["--scan", "--config", config]
        raise ValueError(
            f"Invalid harden mode '{mode}'. Choose from: scan, reinforce, dry-run"
        )

    @staticmethod
    def _describe_request(args: list[str]) -> tuple[str | None, str | None]:
        mode: str | None = None
        config: str | None = None
        has_scan = "--scan" in args
        has_reinforce = "--reinforce" in args
        has_dry_run = "--dry-run" in args

        if has_dry_run:
            mode = "dry-run"
        elif has_reinforce:
            mode = "reinforce"
        elif has_scan:
            mode = "scan"

        for index, arg in enumerate(args):
            if arg == "--config" and index + 1 < len(args):
                config = args[index + 1]
            elif arg.startswith("--config="):
                config = arg.split("=", 1)[1]

        return mode, config

    @staticmethod
    def _build_command(
        args: list[str] | tuple[str, ...], loongshield_path: str | None = None
    ) -> list[str]:
        return [loongshield_path or "loongshield", "seharden", *args]

    @staticmethod
    def _build_result_data(
        raw_args: list[str],
        cmd: list[str],
        tool_path: str | None,
        mode: str | None,
        config: str | None,
    ) -> dict[str, Any]:
        data: dict[str, Any] = {
            "argv": cmd,
            "raw_args": raw_args,
            "tool_path": tool_path,
            "failures": [],
            "fixed_items": [],
        }
        if mode is not None:
            data["mode"] = mode
        if config is not None:
            data["config"] = config
        return data

    @staticmethod
    def _resolve_loongshield_path() -> str | None:
        resolved = shutil.which("loongshield")
        if resolved:
            return resolved

        for candidate in _FALLBACK_LOONGSHIELD_PATHS:
            if os.path.isfile(candidate) and os.access(candidate, os.X_OK):
                return candidate
        return None

    @classmethod
    def _parse_output(cls, clean_output: str, data: dict[str, Any]) -> None:
        for line in reversed(clean_output.splitlines()):
            match = cls._SUMMARY_RE.search(line)
            if match:
                data["passed"] = int(match.group("passed"))
                data["fixed"] = int(match.group("fixed"))
                data["failed"] = int(match.group("failed"))
                data["manual"] = int(match.group("manual"))
                data["dry_run_pending"] = int(match.group("dry_run_pending"))
                data["total"] = int(match.group("total"))
                break

        entries: list[dict[str, str]] = []
        for line in clean_output.splitlines():
            match = _RULE_STATUS_RE.search(line)
            if match:
                entries.append(
                    {
                        "rule_id": match.group("rule_id"),
                        "status": match.group("status"),
                        "message": match.group("message").strip(),
                    }
                )
                continue

            engine_match = _ENGINE_ERROR_RE.search(line)
            if engine_match:
                entries.append(
                    {
                        "rule_id": "",
                        "status": "Engine Error",
                        "message": engine_match.group("message").strip(),
                    }
                )

        mode = data.get("mode")
        fixed_statuses = frozenset({"FAIL", "FAILED"})
        if mode == "reinforce":
            data["failures"] = [
                entry for entry in entries if entry["status"] not in fixed_statuses
            ]
            data["fixed_items"] = [
                entry for entry in entries if entry["status"] in fixed_statuses
            ]
        else:
            data["failures"] = entries

        reported_nonpass = (
            data.get("failed", 0)
            + data.get("manual", 0)
            + data.get("fixed", 0)
            + data.get("dry_run_pending", 0)
        )
        if reported_nonpass > 0 and not data["failures"] and not data["fixed_items"]:
            data["failures"].append(
                {
                    "rule_id": "",
                    "status": "UNKNOWN",
                    "message": (
                        f"Summary reports {reported_nonpass} non-pass rule(s) "
                        "but per-rule details could not be parsed from output."
                    ),
                }
            )
