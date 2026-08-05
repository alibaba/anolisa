"""Contract tests for security observability skill distribution."""

import json
from pathlib import Path


_AGENT_SEC_CORE_DIR = Path(__file__).resolve().parents[3]
_MANIFEST_PATH = _AGENT_SEC_CORE_DIR / "cosh-extension" / "cosh-extension.json"
_COMPONENT_MANIFEST_PATH = _AGENT_SEC_CORE_DIR / ".anolisa" / "component.toml"
_SKILL_PATH = _AGENT_SEC_CORE_DIR / "skills" / "security-observability" / "SKILL.md"


def _parse_frontmatter(content: str) -> dict[str, str]:
    assert content.startswith("---\n")
    frontmatter = content.split("---", 2)[1]
    values: dict[str, str] = {}
    for line in frontmatter.splitlines():
        if ":" not in line:
            continue
        key, value = line.split(":", 1)
        values[key.strip()] = value.strip()
    return values


def test_cosh_extension_does_not_bundle_security_observability_skill() -> None:
    manifest = json.loads(_MANIFEST_PATH.read_text(encoding="utf-8"))

    assert "skills" not in manifest


def test_component_manifest_installs_security_observability_with_skills_bundle() -> None:
    content = _COMPONENT_MANIFEST_PATH.read_text(encoding="utf-8")

    assert 'source = "share/anolisa/skills/"' in content
    assert 'target = "{datadir}/skills/"' in content
    assert _SKILL_PATH.is_file()


def test_security_observability_skill_has_valid_frontmatter() -> None:
    content = _SKILL_PATH.read_text(encoding="utf-8")
    frontmatter = _parse_frontmatter(content)

    assert frontmatter["name"] == "security-observability"
    assert "agent-sec-cli" in frontmatter["description"]
    assert "安全事件" in frontmatter["description"]
    assert "会话" in frontmatter["description"]


def test_security_observability_skill_documents_cli_and_output_contracts() -> None:
    content = _SKILL_PATH.read_text(encoding="utf-8")

    assert "agent-sec-cli events" in content
    assert "agent-sec-cli observability report" in content
    assert "--last-hours" in content
    assert "--since" in content
    assert "--until" in content
    assert "category`、`event_type`、`trace_id`" in content

    event_fields = {
        "event_id",
        "event_type",
        "category",
        "result",
        "timestamp",
        "trace_id",
        "pid",
        "uid",
        "session_id",
        "run_id",
        "call_id",
        "tool_call_id",
        "details",
    }
    report_fields = {
        "first_seen",
        "last_seen",
        "duration_seconds",
        "turn_count",
        "llm_calls",
        "request_bytes",
        "response_bytes",
        "tool_breakdown",
        "security_verdicts",
        "security_hint",
    }

    for field in event_fields | report_fields:
        assert f"`{field}`" in content or f'"{field}"' in content

    assert "backend-specific" in content or "后端专属" in content
    assert "succeeded/failed" in content or "succeeded` / `failed" in content
    assert "pass` / `warn` / `deny" in content
