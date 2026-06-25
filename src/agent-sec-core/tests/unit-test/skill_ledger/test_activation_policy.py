"""Unit tests for Skill Ledger activation policy helpers."""

import pytest
from agent_sec_cli.skill_ledger import activation_policy as policy_module
from agent_sec_cli.skill_ledger.activation_policy import (
    ACTIVATION_POLICIES,
    ACTIVATION_POLICY_ALLOWED_SCAN_STATUSES,
    ACTIVATION_POLICY_LATEST_SCANNED,
    ACTIVATION_POLICY_PASS_ONLY,
    DEFAULT_ACTIVATION_POLICY,
    allowed_scan_statuses_for_policy,
    validate_activation_policy,
)

ACTIVATION_POLICY_PASS_WARN_ONLY = "pass_warn_only"


def test_activation_policies_are_derived_from_status_mapping():
    assert ACTIVATION_POLICIES == frozenset(ACTIVATION_POLICY_ALLOWED_SCAN_STATUSES)


def test_default_activation_policy_is_pass_warn_only():
    assert DEFAULT_ACTIVATION_POLICY == ACTIVATION_POLICY_PASS_WARN_ONLY


def test_pass_warn_only_constant_is_exported():
    assert (
        getattr(policy_module, "ACTIVATION_POLICY_PASS_WARN_ONLY", None)
        == ACTIVATION_POLICY_PASS_WARN_ONLY
    )


@pytest.mark.parametrize(
    "policy",
    [
        ACTIVATION_POLICY_PASS_ONLY,
        ACTIVATION_POLICY_PASS_WARN_ONLY,
        ACTIVATION_POLICY_LATEST_SCANNED,
    ],
)
def test_validate_activation_policy_accepts_and_normalizes_supported_policies(policy):
    assert validate_activation_policy(policy) == ACTIVATION_POLICY_PASS_WARN_ONLY


@pytest.mark.parametrize("policy", ["unknown", ["pass_only"]])
def test_validate_activation_policy_rejects_invalid_policies(policy):
    with pytest.raises(ValueError, match="unsupported activation policy"):
        validate_activation_policy(policy)


def test_allowed_scan_statuses_for_policy():
    for policy in (
        ACTIVATION_POLICY_PASS_ONLY,
        ACTIVATION_POLICY_PASS_WARN_ONLY,
        ACTIVATION_POLICY_LATEST_SCANNED,
    ):
        assert allowed_scan_statuses_for_policy(policy) == frozenset({"pass", "warn"})
