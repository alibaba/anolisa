# -*- coding: utf-8 -*-
from __future__ import annotations

from sysom_cli.core.registry import command_metadata
from sysom_cli.lib.specialty_args import SPECIALTY_INVOKE_ARGS
from sysom_cli.lib.specialty_command import BaseServiceSpecialtyCommand


@command_metadata(
    name="loadtask",
    help="SysOM 专项 loadtask：系统负载诊断。params 见 references/diagnoses/loadtask.md",
    subsystem="load",
    args=list(SPECIALTY_INVOKE_ARGS),
)
class LoadtaskCommand(BaseServiceSpecialtyCommand):
    SERVICE_NAME = "loadtask"
