"""Shared daemon unit-test fixtures."""

import pytest


@pytest.fixture(autouse=True)
def disable_prompt_preload(monkeypatch):
    """Prevent daemon unit tests from downloading/loading the prompt model."""
    monkeypatch.setenv("AGENT_SEC_DAEMON_PROMPT_PRELOAD", "0")
