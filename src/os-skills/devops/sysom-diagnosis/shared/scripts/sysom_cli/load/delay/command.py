# -*- coding: utf-8 -*-
from __future__ import annotations

from sysom_cli.core.registry import command_metadata
from sysom_cli.lib.specialty_args import SPECIALTY_INVOKE_ARGS
from sysom_cli.lib.specialty_command import BaseServiceSpecialtyCommand


@command_metadata(
    name="delay",
    help="SysOM 专项 delay：调度延迟（nosched）。params 见 references/diagnoses/delay.md",
    subsystem="load",
    args=list(SPECIALTY_INVOKE_ARGS),
)
class DelayCommand(BaseServiceSpecialtyCommand):
    SERVICE_NAME = "delay"
