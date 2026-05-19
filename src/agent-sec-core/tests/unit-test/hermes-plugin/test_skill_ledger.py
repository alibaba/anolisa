"""Unit tests for hermes-plugin skill_ledger capability."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from unittest.mock import patch

import pytest

_HERMES_PLUGIN_DIR = Path(__file__).resolve().parents[3] / "hermes-plugin"
sys.path.insert(0, str(_HERMES_PLUGIN_DIR))

from src.capabilities.skill_ledger import SkillLedgerCapability  # noqa: E402
from src.cli_runner import CliResult  # noqa: E402


def _make_capability(
    root: Path,
    *,
    enable_block: bool = False,
    block_statuses: list[str] | None = None,
    max_warnings_per_turn: int | str = 5,
    max_warning_contexts: int | str = 128,
) -> SkillLedgerCapability:
    cap = SkillLedgerCapability()
    cap._timeout = 5.0
    cap._on_register(
        {
            "enable_block": enable_block,
            "block_statuses": block_statuses or ["none", "drifted", "deny", "tampered"],
            "max_warnings_per_turn": max_warnings_per_turn,
            "max_warning_contexts": max_warning_contexts,
        }
    )
    cap._skills_dir = root
    return cap


def _make_skill(
    root: Path,
    rel: str,
    *,
    frontmatter_name: str | None = None,
) -> Path:
    skill_dir = root / rel
    skill_dir.mkdir(parents=True, exist_ok=True)
    name = frontmatter_name or skill_dir.name
    (skill_dir / "SKILL.md").write_text(
        f"---\nname: {name}\ndescription: Test skill\n---\nBody\n",
        encoding="utf-8",
    )
    return skill_dir


def _cli_status(status: str, *, exit_code: int = 0) -> CliResult:
    return CliResult(
        stdout=json.dumps({"status": status}), stderr="", exit_code=exit_code
    )


class TestSkillLedgerHooks:
    """Behavior tests for pre_tool_call and transform_llm_output."""

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_pass_allows_without_warning(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/pass-skill")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status("pass")

        result = cap._on_pre_tool_call(
            "skill_view", {"name": "pass-skill"}, session_id="s1"
        )

        assert result is None
        assert (
            cap._on_transform_llm_output("assistant response", session_id="s1") is None
        )

    @pytest.mark.parametrize(
        "status",
        ["none", "warn", "drifted", "deny", "tampered", "error", "unknown"],
    )
    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_non_pass_default_allows_and_prepends_warning(
        self, mock_cli, tmp_path, status
    ):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status(status, exit_code=1)

        result = cap._on_pre_tool_call("skill_view", {"name": "risky"}, task_id="t1")
        output = cap._on_transform_llm_output("assistant response", task_id="t1")

        assert result is None
        assert output.startswith("[agent-sec-core skill-ledger warning]")
        assert f"status={status}" in output
        assert output.endswith("assistant response")

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_enable_block_blocks_configured_status_without_warning(
        self, mock_cli, tmp_path
    ):
        root = tmp_path / "skills"
        _make_skill(root, "security/blocked")
        cap = _make_capability(root, enable_block=True)
        mock_cli.return_value = _cli_status("deny", exit_code=1)

        result = cap._on_pre_tool_call("skill_view", {"name": "blocked"}, run_id="r1")

        assert result is not None
        assert result["action"] == "block"
        assert "status=deny" in result["message"]
        assert cap._on_transform_llm_output("assistant response", run_id="r1") is None

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_enable_block_allows_unconfigured_status_without_warning(
        self, mock_cli, tmp_path
    ):
        root = tmp_path / "skills"
        _make_skill(root, "security/warn-only")
        cap = _make_capability(root, enable_block=True)
        mock_cli.return_value = _cli_status("warn")

        result = cap._on_pre_tool_call("skill_view", {"name": "warn-only"})

        assert result is None
        assert cap._on_transform_llm_output("assistant response") is None

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_nonzero_exit_with_valid_json_still_uses_status(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/drifted")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status("drifted", exit_code=1)

        cap._on_pre_tool_call("skill_view", {"name": "drifted"}, session_id="s1")
        output = cap._on_transform_llm_output("assistant response", session_id="s1")

        assert "status=drifted" in output

    @pytest.mark.parametrize(
        "cli_result",
        [
            CliResult(stdout="", stderr="timeout", exit_code=124),
            CliResult(stdout="not-json", stderr="", exit_code=0),
        ],
    )
    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_cli_failure_paths_fail_open(self, mock_cli, tmp_path, cli_result):
        root = tmp_path / "skills"
        _make_skill(root, "devops/flaky")
        cap = _make_capability(root)
        mock_cli.return_value = cli_result

        result = cap._on_pre_tool_call("skill_view", {"name": "flaky"})

        assert result is None
        assert cap._on_transform_llm_output("assistant response") is None

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_unresolved_skill_fails_open_without_cli(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        cap = _make_capability(root)

        result = cap._on_pre_tool_call("skill_view", {"name": "missing"})

        assert result is None
        mock_cli.assert_not_called()

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_warning_context_cache_is_bounded(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root)
        cap._max_warning_contexts = 2
        mock_cli.return_value = _cli_status("warn")

        for idx in range(3):
            cap._on_pre_tool_call(
                "skill_view",
                {"name": "risky"},
                session_id=f"s{idx}",
            )

        assert len(cap._warnings_by_context) == 2
        assert "session_id:s0" not in cap._warnings_by_context

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_warning_without_context_is_not_injected_into_later_session(
        self, mock_cli, tmp_path
    ):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status("warn")

        cap._on_pre_tool_call("skill_view", {"name": "risky"})
        output = cap._on_transform_llm_output("assistant response", session_id="s1")

        assert output is None
        assert cap._warnings_by_context == {}

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_warning_with_context_is_not_consumed_by_contextless_transform(
        self, mock_cli, tmp_path
    ):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status("warn")

        cap._on_pre_tool_call("skill_view", {"name": "risky"}, session_id="s1")

        assert cap._on_transform_llm_output("assistant response") is None
        output = cap._on_transform_llm_output("assistant response", session_id="s1")
        assert output.startswith("[agent-sec-core skill-ledger warning]")

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_zero_max_warnings_disables_visible_injection(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root, max_warnings_per_turn=0)
        mock_cli.return_value = _cli_status("warn")

        cap._on_pre_tool_call("skill_view", {"name": "risky"}, session_id="s1")

        assert (
            cap._on_transform_llm_output("assistant response", session_id="s1") is None
        )
        assert cap._warnings_by_context == {}

    def test_invalid_warning_config_uses_safe_defaults(self, tmp_path):
        root = tmp_path / "skills"
        cap = _make_capability(
            root,
            max_warnings_per_turn="invalid",
            max_warning_contexts="invalid",
        )

        assert cap._max_warnings_per_turn == 5
        assert cap._max_warning_contexts == 128

    def test_negative_warning_config_clamps_to_minimum(self, tmp_path):
        root = tmp_path / "skills"
        cap = _make_capability(
            root,
            max_warnings_per_turn=-1,
            max_warning_contexts=-1,
        )

        assert cap._max_warnings_per_turn == 0
        assert cap._max_warning_contexts == 1


class TestSkillResolution:
    """Hermes local skill name resolution tests."""

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_resolves_by_category_name(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        skill_dir = _make_skill(root, "mlops/axolotl")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status("pass")

        cap._on_pre_tool_call("skill_view", {"name": "mlops/axolotl"})

        assert mock_cli.call_args[0][0][-1] == str(skill_dir.resolve())

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_frontmatter_name_is_not_used_for_resolution(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(
            root,
            "directory-name",
            frontmatter_name="frontmatter-name",
        )
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status("pass")

        cap._on_pre_tool_call("skill_view", {"skill_name": "frontmatter-name"})

        mock_cli.assert_not_called()

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_supporting_file_path_does_not_override_name(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        skill_dir = _make_skill(root, "tools/name-wins")
        other_dir = _make_skill(root, "tools/ignored-path")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status("pass")

        cap._on_pre_tool_call(
            "skill_view",
            {
                "name": "name-wins",
                "file_path": str(other_dir / "SKILL.md"),
            },
        )

        assert mock_cli.call_args[0][0][-1] == str(skill_dir.resolve())

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_file_path_without_name_fails_open(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "tools/relative")
        cap = _make_capability(root)

        result = cap._on_pre_tool_call(
            "skill_view",
            {"file_path": "SKILL.md"},
        )

        assert result is None
        mock_cli.assert_not_called()

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_ignored_internal_dirs_are_not_resolved(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, ".archive/hidden")
        cap = _make_capability(root)

        result = cap._on_pre_tool_call("skill_view", {"name": "hidden"})

        assert result is None
        mock_cli.assert_not_called()

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_ambiguous_bare_name_fails_open_without_cli(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/duplicate")
        _make_skill(root, "security/duplicate")
        cap = _make_capability(root)

        result = cap._on_pre_tool_call("skill_view", {"name": "duplicate"})

        assert result is None
        mock_cli.assert_not_called()

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_qualified_plugin_style_name_is_skipped(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "plugin/skill")
        cap = _make_capability(root)

        result = cap._on_pre_tool_call("skill_view", {"name": "plugin:skill"})

        assert result is None
        mock_cli.assert_not_called()
