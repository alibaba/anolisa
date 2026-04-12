# -*- coding: utf-8 -*-
"""非内存 `io`/`net`/`load` 子命令的 OpenAPI 侧参数（与 DiagnosisBackend/invoke 实现同源；不含 --service-name，由子命令固定）。"""
from __future__ import annotations

from typing import Any, Dict, List, Tuple

SPECIALTY_INVOKE_ARGS: List[Tuple[Any, Dict[str, Any]]] = [
    (
        ["--verbose-envelope"],
        {
            "action": "store_true",
            "help": "输出展开型 agent.summary（默认紧凑：省 token，结论见 data.remote）。",
        },
    ),
    (
        ["--channel"],
        {"default": "ecs", "help": "诊断通道，官方文档当前固定为 ecs"},
    ),
    (
        ["--params"],
        {
            "default": None,
            "help": "params JSON 字符串（与 OpenAPI 文档「诊断参数说明」一致）",
        },
    ),
    (
        ["--params-file"],
        {"default": None, "help": "params JSON 文件路径"},
    ),
    (
        ["--instance"],
        {
            "default": None,
            "help": "合并到 params.instance；省略时若在 ECS 上可自动从元数据 instance-id 补全",
        },
    ),
    (
        ["--region"],
        {
            "default": None,
            "help": "合并到 params.region；省略时若在 ECS 上可自动从元数据 region-id 补全",
        },
    ),
    (
        ["--timeout"],
        {"type": int, "default": 300, "help": "轮询 GetDiagnosisResult 总超时秒数"},
    ),
    (
        ["--poll-interval"],
        {"type": int, "default": 1, "help": "轮询间隔秒数"},
    ),
]
