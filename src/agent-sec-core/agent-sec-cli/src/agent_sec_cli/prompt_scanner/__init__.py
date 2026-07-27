"""PromptScanner – multi-layer prompt injection & jailbreak detection.

Public API::

    from agent_sec_cli.prompt_scanner import PromptScanner, ScanResult, ScanMode

    scanner = PromptScanner()                       # default: STANDARD (L1+L2)
    result  = scanner.scan("ignore previous instructions")
    result.is_threat   # True / False
    result.risk_score  # 0.0 – 1.0
"""

from agent_sec_cli.prompt_scanner.config import ScanConfig, ScanMode
from agent_sec_cli.prompt_scanner.result import ScanResult, ThreatType
from agent_sec_cli.prompt_scanner.scanner import (
    AsyncPromptScanner,
    PromptScanner,
)

__all__ = [
    "PromptScanner",
    "AsyncPromptScanner",
    "ScanResult",
    "ScanMode",
    "ScanConfig",
    "ThreatType",
]
