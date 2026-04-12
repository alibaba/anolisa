# -*- coding: utf-8 -*-
from __future__ import annotations

from sysom_cli.core.registry import command_metadata
from sysom_cli.lib.specialty_args import SPECIALTY_INVOKE_ARGS
from sysom_cli.lib.specialty_command import BaseServiceSpecialtyCommand


@command_metadata(
    name="packetdrop",
    help="SysOM 专项 packetdrop：网络丢包诊断。params 见 references/diagnoses/packetdrop.md",
    subsystem="net",
    args=list(SPECIALTY_INVOKE_ARGS),
)
class PacketdropCommand(BaseServiceSpecialtyCommand):
    SERVICE_NAME = "packetdrop"
