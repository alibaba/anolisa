"""Unit tests for hermes-plugin skill_ledger capability."""

from __future__ import annotations

import json
import logging
import sys
import tomllib
from pathlib import Path
from types import ModuleType
from unittest.mock import patch

import pytest

_HERMES_PLUGIN_DIR = Path(__file__).resolve().parents[3] / "hermes-plugin"
sys.path.insert(0, str(_HERMES_PLUGIN_DIR))

from src.capabilities.skill_ledger import SkillLedgerCapability  # noqa: E402
from src.cli_runner import CliResult  # noqa: E402
from src.registry import load_config, register_capabilities  # noqa: E402

_DEFAULT_MESSAGE = object()


def _make_capability(
    root: Path,
    *,
    policy: str = "ask",
    include_policy: bool = True,
    enable_block: bool = False,
    block_statuses: list[str] | None = None,
    max_warnings_per_turn: int | str = 5,
    max_warning_contexts: int | str = 128,
) -> SkillLedgerCapability:
    cap = SkillLedgerCapability()
    cap._timeout = 5.0
    config = {
        "enable_block": enable_block,
        "block_statuses": block_statuses or ["none", "drifted", "deny", "tampered"],
        "max_warnings_per_turn": max_warnings_per_turn,
        "max_warning_contexts": max_warning_contexts,
    }
    if include_policy:
        config["policy"] = policy
    cap._on_register(config)
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


def _cli_status(
    status: str,
    *,
    exit_code: int = 0,
    message: str | None | object = _DEFAULT_MESSAGE,
    user_decision: dict | None = None,
) -> CliResult:
    if message is _DEFAULT_MESSAGE:
        message = None if status == "pass" else f"summary message for status={status}"
    if not isinstance(message, str):
        message = None
    return CliResult(
        stdout=json.dumps(
            {
                "latestStatus": status,
                "message": message,
                "userDecision": user_decision,
            }
        ),
        stderr="",
        exit_code=exit_code,
    )


def _install_gateway_session_context(monkeypatch, session_id: str) -> None:
    gateway_module = ModuleType("gateway")
    session_context_module = ModuleType("gateway.session_context")

    def get_session_env(name: str, default: str = "") -> str:
        assert name == "HERMES_SESSION_ID"
        return session_id or default

    session_context_module.get_session_env = get_session_env
    gateway_module.session_context = session_context_module
    monkeypatch.setitem(sys.modules, "gateway", gateway_module)
    monkeypatch.setitem(sys.modules, "gateway.session_context", session_context_module)


class _RecordingHermesContext:
    def __init__(self) -> None:
        self.hooks: list[str] = []

    def register_hook(self, hook_name, _callback):
        self.hooks.append(hook_name)


def test_default_config_enables_skill_ledger_ask_policy():
    """Default Hermes installs request user attention without hard blocking."""
    config = tomllib.loads(
        (_HERMES_PLUGIN_DIR / "src" / "config.toml").read_text(encoding="utf-8")
    )

    assert config["capabilities"]["skill-ledger"]["enabled"] is True
    assert config["capabilities"]["skill-ledger"]["policy"] == "ask"


def test_default_config_registers_skill_ledger_hooks():
    ctx = _RecordingHermesContext()
    config = load_config(_HERMES_PLUGIN_DIR / "src")

    register_capabilities(ctx, [SkillLedgerCapability()], config)

    assert ctx.hooks == ["pre_tool_call", "transform_llm_output"]


class TestSkillLedgerHooks:
    """Behavior tests for pre_tool_call and transform_llm_output."""

    @pytest.mark.parametrize(
        "sentinel",
        [
            ".skillfs-inbox",
            "skill-discover/SKILL.md",
        ],
    )
    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_skillfs_inplace_sentinel_does_not_bypass_ledger(
        self, mock_cli, tmp_path, sentinel
    ):
        root = tmp_path / "skills"
        skill_dir = _make_skill(root, "devops/risky")
        sentinel_path = root / sentinel
        if sentinel_path.name == "SKILL.md":
            sentinel_path.parent.mkdir(parents=True)
            sentinel_path.write_text(
                "---\nname: skill-discover\n---\n", encoding="utf-8"
            )
        else:
            sentinel_path.mkdir(parents=True)
        cap = _make_capability(root, policy="warn")
        mock_cli.return_value = _cli_status("pass")

        result = cap._on_pre_tool_call(
            "skill_view", {"name": "devops/risky"}, session_id="s1"
        )
        output = cap._on_transform_llm_output(
            response_text="assistant response", session_id="s1"
        )

        assert result is None
        mock_cli.assert_called_once_with(
            ["skill-ledger", "show", str(skill_dir.resolve())],
            timeout=5.0,
            trace_context={"agent_name": "hermes", "session_id": "s1"},
        )
        assert output is None

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_skillfs_inplace_root_debug_policy_only_logs(
        self, mock_cli, tmp_path, caplog
    ):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        (root / ".skillfs-inbox").mkdir()
        cap = _make_capability(root, policy="debug")
        mock_cli.return_value = _cli_status("warn")
        caplog.set_level(logging.DEBUG, logger="agent-sec-core")

        result = cap._on_pre_tool_call("skill_view", {"name": "risky"}, session_id="s1")
        output = cap._on_transform_llm_output(
            response_text="assistant response", session_id="s1"
        )

        assert result is None
        mock_cli.assert_called_once()
        assert output is None
        assert not cap._warnings_by_context
        assert not any(
            "暂不支持Hermes场景" in record.message for record in caplog.records
        )

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_skillfs_inplace_root_block_policy_uses_ledger_result(
        self, mock_cli, tmp_path
    ):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        (root / ".skillfs-inbox").mkdir()
        cap = _make_capability(root, policy="block")
        mock_cli.return_value = _cli_status("warn")

        result = cap._on_pre_tool_call("skill_view", {"name": "risky"}, session_id="s1")
        output = cap._on_transform_llm_output(
            response_text="assistant response", session_id="s1"
        )

        assert result["action"] == "block"
        mock_cli.assert_called_once()
        assert output is None

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_hidden_nested_skill_is_forwarded_as_canonical_path(
        self, mock_cli, tmp_path
    ):
        root = tmp_path / "skills"
        root.mkdir()
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status("pass")

        result = cap._on_pre_tool_call(
            "skill_view",
            {"name": "apple/apple-notes"},
            session_id="s1",
        )

        assert result is None
        mock_cli.assert_called_once_with(
            ["skill-ledger", "show", str(root / "apple" / "apple-notes")],
            timeout=5.0,
            trace_context={"agent_name": "hermes", "session_id": "s1"},
        )

    @patch("src.capabilities.skill_ledger.Path.rglob")
    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_skill_file_traversal_loop_fails_open_with_short_notice(
        self, mock_cli, mock_rglob, tmp_path
    ):
        root = tmp_path / "skills"
        root.mkdir()
        cap = _make_capability(root, policy="warn")
        mock_rglob.side_effect = OSError("File system loop detected")

        result = cap._on_pre_tool_call("skill_view", {"name": "risky"}, session_id="s1")
        output = cap._on_transform_llm_output(
            response_text="assistant response", session_id="s1"
        )

        assert result is None
        mock_cli.assert_not_called()
        assert (
            output
            == "暂不支持Hermes场景，请自行关注skill安全性。\n\nassistant response"
        )

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

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_warn_allows_without_warning(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/warn-skill")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status("warn", message=None)

        result = cap._on_pre_tool_call(
            "skill_view", {"name": "warn-skill"}, session_id="s1"
        )

        assert result is None
        assert (
            cap._on_transform_llm_output("assistant response", session_id="s1") is None
        )

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_user_decision_summary_allows_without_warning(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/allowed-risk")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status(
            "deny",
            exit_code=0,
            message=None,
            user_decision={"action": "allow"},
        )

        result = cap._on_pre_tool_call(
            "skill_view", {"name": "allowed-risk"}, session_id="s1"
        )

        assert result is None
        assert (
            cap._on_transform_llm_output("assistant response", session_id="s1") is None
        )

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_passes_hermes_trace_context_to_cli(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/pass-skill")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status("pass")

        result = cap._on_pre_tool_call(
            "skill_view",
            {"name": "pass-skill"},
            session_id="session-1",
            tool_call_id="tool-1",
        )

        assert result is None
        assert mock_cli.call_args.kwargs["trace_context"] == {
            "agent_name": "hermes",
            "session_id": "session-1",
            "tool_call_id": "tool-1",
        }
        assert "run_id" not in mock_cli.call_args.kwargs["trace_context"]

    @pytest.mark.parametrize(
        "status",
        ["none", "drifted", "deny", "tampered", "error", "unknown"],
    )
    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_non_pass_default_ask_policy_falls_back_to_warning(
        self, mock_cli, tmp_path, status, caplog
    ):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root)
        mock_cli.return_value = _cli_status(status, exit_code=1)
        caplog.set_level(logging.DEBUG, logger="agent-sec-core")

        result = cap._on_pre_tool_call("skill_view", {"name": "risky"}, task_id="t1")
        output = cap._on_transform_llm_output(
            response_text="assistant response", task_id="t1"
        )

        assert result is None
        assert output.startswith("[agent-sec-core skill-ledger warning]")
        assert f"status={status}" in output
        assert any(f"status={status}" in record.message for record in caplog.records)

    @pytest.mark.parametrize("status", ["deny", "drifted"])
    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_debug_policy_logs_and_allows(self, mock_cli, tmp_path, status, caplog):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root, policy="debug")
        mock_cli.return_value = _cli_status(status, exit_code=1)
        caplog.set_level(logging.DEBUG, logger="agent-sec-core")

        result = cap._on_pre_tool_call("skill_view", {"name": "risky"}, task_id="t1")
        output = cap._on_transform_llm_output(
            response_text="assistant response", task_id="t1"
        )

        assert result is None
        assert output is None
        assert not cap._warnings_by_context
        assert any(f"status={status}" in record.message for record in caplog.records)
        assert not any(record.levelno >= logging.WARNING for record in caplog.records)

    @pytest.mark.parametrize(
        "status",
        ["none", "warn", "drifted", "deny", "tampered", "error", "unknown"],
    )
    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_warn_policy_allows_and_prepends_warning(self, mock_cli, tmp_path, status):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root, policy="warn")
        mock_cli.return_value = _cli_status(status, exit_code=1)

        result = cap._on_pre_tool_call("skill_view", {"name": "risky"}, task_id="t1")
        output = cap._on_transform_llm_output(
            response_text="assistant response", task_id="t1"
        )

        assert result is None
        assert output.startswith("[agent-sec-core skill-ledger warning]")
        assert f"status={status}" in output
        assert output.endswith("assistant response")

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_legacy_enable_block_false_maps_to_warn(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root, include_policy=False, enable_block=False)
        mock_cli.return_value = _cli_status("warn")

        result = cap._on_pre_tool_call("skill_view", {"name": "risky"}, task_id="t1")
        output = cap._on_transform_llm_output(
            response_text="assistant response", task_id="t1"
        )

        assert result is None
        assert output.startswith("[agent-sec-core skill-ledger warning]")
        assert "status=warn" in output

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_legacy_enable_block_true_maps_to_block(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "security/blocked")
        cap = _make_capability(root, include_policy=False, enable_block=True)
        mock_cli.return_value = _cli_status("deny", exit_code=1)

        result = cap._on_pre_tool_call("skill_view", {"name": "blocked"}, run_id="r1")

        assert result is not None
        assert result["action"] == "block"
        assert "status=deny" in result["message"]

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_drifted_warning_uses_response_text_and_logs_status(
        self, mock_cli, tmp_path, caplog
    ):
        root = tmp_path / "skills"
        _make_skill(root, "devops/drifted")
        cap = _make_capability(root, policy="warn")
        mock_cli.return_value = _cli_status("drifted", exit_code=1)
        caplog.set_level(logging.WARNING, logger="agent-sec-core")

        result = cap._on_pre_tool_call(
            "skill_view", {"name": "drifted"}, session_id="s1"
        )
        output = cap._on_transform_llm_output(
            response_text="assistant response", session_id="s1"
        )

        assert result is None
        assert output.startswith("[agent-sec-core skill-ledger warning]")
        assert "status=drifted" in output
        assert output.endswith("assistant response")
        assert any("status=drifted" in record.message for record in caplog.records)

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_runtime_hermes_session_context_bridges_missing_pre_session_id(
        self, mock_cli, tmp_path, monkeypatch
    ):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root, policy="warn")
        mock_cli.return_value = _cli_status("drifted", exit_code=1)
        _install_gateway_session_context(monkeypatch, "hermes-session-1")

        cap._on_pre_tool_call(
            "skill_view", {"name": "risky"}, session_id="", task_id="t1"
        )

        assert list(cap._warnings_by_context) == ["session_id:hermes-session-1"]
        output = cap._on_transform_llm_output(
            response_text="assistant response", session_id="hermes-session-1"
        )
        second = cap._on_transform_llm_output(
            response_text="assistant response", session_id="hermes-session-1"
        )

        assert output.startswith("[agent-sec-core skill-ledger warning]")
        assert output.endswith("assistant response")
        assert second is None

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_mismatched_keys_do_not_use_pending_warning_fallback(
        self, mock_cli, tmp_path, monkeypatch
    ):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root, policy="warn")
        mock_cli.return_value = _cli_status("drifted", exit_code=1)
        _install_gateway_session_context(monkeypatch, "")

        cap._on_pre_tool_call(
            "skill_view", {"name": "risky"}, session_id="", task_id="t1"
        )
        output = cap._on_transform_llm_output(
            response_text="assistant response", session_id="s1"
        )

        assert output is None
        assert list(cap._warnings_by_context) == ["task_id:t1"]

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_block_policy_blocks_message_without_warning(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "security/blocked")
        cap = _make_capability(root, policy="block")
        mock_cli.return_value = _cli_status("deny", exit_code=1)

        result = cap._on_pre_tool_call("skill_view", {"name": "blocked"}, run_id="r1")

        assert result is not None
        assert result["action"] == "block"
        assert "status=deny" in result["message"]
        assert cap._on_transform_llm_output("assistant response", run_id="r1") is None

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_block_policy_allows_when_summary_has_no_message(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "security/warn-only")
        cap = _make_capability(root, policy="block")
        mock_cli.return_value = _cli_status("warn", message=None)

        result = cap._on_pre_tool_call("skill_view", {"name": "warn-only"})

        assert result is None
        assert cap._on_transform_llm_output("assistant response") is None

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_nonzero_exit_with_valid_json_still_uses_status(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/drifted")
        cap = _make_capability(root, policy="warn")
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
        cap = _make_capability(root, policy="warn")
        mock_cli.return_value = cli_result

        result = cap._on_pre_tool_call("skill_view", {"name": "flaky"})

        assert result is None
        assert cap._on_transform_llm_output("assistant response") is None

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_unresolved_skill_fails_open_without_cli(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        cap = _make_capability(root, policy="warn")

        result = cap._on_pre_tool_call("skill_view", {"name": "missing"})

        assert result is None
        mock_cli.assert_not_called()

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_warning_context_cache_is_bounded(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root, policy="warn")
        cap._max_warning_contexts = 2
        mock_cli.return_value = _cli_status("drifted")

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
        cap = _make_capability(root, policy="warn")
        mock_cli.return_value = _cli_status("drifted")

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
        cap = _make_capability(root, policy="warn")
        mock_cli.return_value = _cli_status("drifted")

        cap._on_pre_tool_call("skill_view", {"name": "risky"}, session_id="s1")

        assert cap._on_transform_llm_output("assistant response") is None
        output = cap._on_transform_llm_output("assistant response", session_id="s1")
        assert output.startswith("[agent-sec-core skill-ledger warning]")

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_zero_max_warnings_disables_visible_injection(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        _make_skill(root, "devops/risky")
        cap = _make_capability(root, policy="warn", max_warnings_per_turn=0)
        mock_cli.return_value = _cli_status("drifted")

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
    def test_symlink_skills_root_preserves_canonical_path(self, mock_cli, tmp_path):
        physical_root = tmp_path / "physical-skills"
        _make_skill(physical_root, "mlops/axolotl")
        canonical_root = tmp_path / "skills"
        canonical_root.symlink_to(physical_root, target_is_directory=True)
        cap = _make_capability(canonical_root)
        mock_cli.return_value = _cli_status("pass")

        cap._on_pre_tool_call("skill_view", {"name": "mlops/axolotl"})

        canonical_skill_dir = canonical_root / "mlops" / "axolotl"
        assert mock_cli.call_args[0][0][-1] == str(canonical_skill_dir)
        assert canonical_skill_dir != canonical_skill_dir.resolve()

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
    def test_qualified_internal_dir_is_not_resolved(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        root.mkdir()
        cap = _make_capability(root)

        result = cap._on_pre_tool_call(
            "skill_view",
            {"name": ".archive/hidden"},
        )

        assert result is None
        mock_cli.assert_not_called()

    @patch("src.capabilities.skill_ledger.call_agent_sec_cli")
    def test_qualified_symlink_escape_is_not_resolved(self, mock_cli, tmp_path):
        root = tmp_path / "skills"
        outside = tmp_path / "outside"
        root.mkdir()
        outside.mkdir()
        (root / "linked").symlink_to(outside, target_is_directory=True)
        cap = _make_capability(root)

        result = cap._on_pre_tool_call(
            "skill_view",
            {"name": "linked/hidden"},
        )

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
