"""Capability registry — exports all available security capabilities."""

from __future__ import annotations

from .code_scan import CodeScanCapability
from .observability import ObservabilityCapability

ALL_CAPABILITIES = [CodeScanCapability(), ObservabilityCapability()]
