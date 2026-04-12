# -*- coding: utf-8 -*-
from __future__ import annotations

from sysom_cli.core.registry import command_metadata
from sysom_cli.lib.specialty_args import SPECIALTY_INVOKE_ARGS
from sysom_cli.lib.specialty_command import BaseServiceSpecialtyCommand


@command_metadata(
    name="netjitter",
    help="SysOM 专项 netjitter：网络抖动诊断。params 见 references/diagnoses/netjitter.md",
    subsystem="net",
    args=list(SPECIALTY_INVOKE_ARGS),
)
class NetjitterCommand(BaseServiceSpecialtyCommand):
    SERVICE_NAME = "netjitter"
