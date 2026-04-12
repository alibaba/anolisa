# -*- coding: utf-8 -*-
from __future__ import annotations

from argparse import Namespace
from typing import Any, Dict

from sysom_cli.core.base import ExecutionMode, RemoteOnlyCommand
from sysom_cli.lib.diagnosis_backend import get_diagnosis_backend


class BaseServiceSpecialtyCommand(RemoteOnlyCommand):
    """固定 service_name 的 SysOM 专项；远程-only，不走 MEMORY_MODE。"""

    SERVICE_NAME: str = ""

    @property
    def command_name(self) -> str:
        return self.SERVICE_NAME

    def execute_remote(self, ns: Namespace) -> Dict[str, Any]:
        return get_diagnosis_backend().invoke_specialty(self.SERVICE_NAME, ns)

    @property
    def supported_modes(self) -> Dict[str, bool]:
        return {
            ExecutionMode.LOCAL: False,
            ExecutionMode.REMOTE: True,
            ExecutionMode.HYBRID: False,
        }
