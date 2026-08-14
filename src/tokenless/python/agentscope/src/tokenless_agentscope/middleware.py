"""Backward-compatible AgentScope 2.x middleware imports."""

from tokenless_agentscope import _v2

CompressionMode = _v2.CompressionMode
TokenlessMiddleware = _v2.TokenlessMiddleware
_SKIP_TOOLS = _v2._SKIP_TOOLS
_SHELL_TOOLS = _v2._SHELL_TOOLS
_CONSERVATIVE_THRESHOLDS = _v2._CONSERVATIVE_THRESHOLDS
_SHELL_THRESHOLDS = _v2._SHELL_THRESHOLDS
_AGGRESSIVE_THRESHOLDS = _v2._AGGRESSIVE_THRESHOLDS

__all__ = ["CompressionMode", "TokenlessMiddleware"]
