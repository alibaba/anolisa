# -*- coding: utf-8 -*-
from __future__ import annotations

from sysom_cli.core.registry import command_metadata
from sysom_cli.lib.specialty_args import SPECIALTY_INVOKE_ARGS
from sysom_cli.lib.specialty_command import BaseServiceSpecialtyCommand


@command_metadata(
    name="iodiagnose",
    help="SysOM 专项 iodiagnose：IO 深度/一键诊断。params 见 references/diagnoses/iodiagnose.md",
    subsystem="io",
    args=list(SPECIALTY_INVOKE_ARGS),
)
class IodiagnoseCommand(BaseServiceSpecialtyCommand):
    SERVICE_NAME = "iodiagnose"
