"""Capability registry — exports all available security capabilities."""

from __future__ import annotations

from .code_scan import CodeScanCapability
from .observability import ObservabilityCapability
from .pii_scan import PiiScanCapability

ALL_CAPABILITIES = [
    CodeScanCapability(),
    ObservabilityCapability(),
    PiiScanCapability(),
]
