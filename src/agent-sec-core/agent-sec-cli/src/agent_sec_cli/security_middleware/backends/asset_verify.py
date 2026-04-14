"""Asset-verify backend — delegates to the asset_verify package."""

from __future__ import annotations

from agent_sec_cli.asset_verify import run_verification
from agent_sec_cli.security_middleware.result import ActionResult


class AssetVerifyBackend:
    """Verify skill integrity using the asset_verify module."""

    def execute(
        self,
        ctx,
        skill: str | None = None,
        **kwargs,
    ) -> ActionResult:
        """Run verification for a single skill or all configured directories.

        Args:
            ctx:   Request context (unused beyond tracing).
            skill: Optional path to a single skill directory to verify.
                   When *None*, all directories from ``config.conf`` are scanned.
        """
        try:
            results = run_verification(skill)
        except Exception as exc:
            return ActionResult(
                success=False,
                error=f"Verification error: {exc}",
                exit_code=1,
            )

        passed = results["passed"]
        failed = results["failed"]

        # Build human-readable output
        output_lines: list[str] = []
        for name in passed:
            output_lines.append(f"[OK] {name}")
        for item in failed:
            output_lines.append(f"[ERROR] {item['name']}")
            output_lines.append(f"  {item['error']}")

        output_lines.append("")
        output_lines.append("=" * 50)
        output_lines.append(f"PASSED: {len(passed)}")
        output_lines.append(f"FAILED: {len(failed)}")
        output_lines.append("=" * 50)
        status = "VERIFICATION PASSED" if not failed else "VERIFICATION FAILED"
        output_lines.append(status)

        has_failures = len(failed) > 0

        return ActionResult(
            success=(not has_failures),
            stdout="\n".join(output_lines) + "\n",
            data={"passed": len(passed), "failed": len(failed)},
            exit_code=1 if has_failures else 0,
        )
