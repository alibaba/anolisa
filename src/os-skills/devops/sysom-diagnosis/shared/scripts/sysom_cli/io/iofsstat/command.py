# -*- coding: utf-8 -*-
from __future__ import annotations

from sysom_cli.core.registry import command_metadata
from sysom_cli.lib.specialty_args import SPECIALTY_INVOKE_ARGS
from sysom_cli.lib.specialty_command import BaseServiceSpecialtyCommand


@command_metadata(
    name="iofsstat",
    help="SysOM 专项 iofsstat：磁盘 IO 流量/大盘。params 见 references/diagnoses/iofsstat.md",
    subsystem="io",
    args=list(SPECIALTY_INVOKE_ARGS),
)
class IofsstatCommand(BaseServiceSpecialtyCommand):
    SERVICE_NAME = "iofsstat"
