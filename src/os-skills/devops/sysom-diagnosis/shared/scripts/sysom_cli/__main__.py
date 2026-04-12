# -*- coding: utf-8 -*-
"""
入口：python -m sysom_cli [--list-capabilities] [top_cmd] [sub_cmd] ...
"""
from __future__ import annotations

import argparse
import sys
from argparse import Namespace
from typing import Any, Dict, List


# ============================================================================
# 命令配置和解析器构建
# ============================================================================

# 二层命令配置：(name, help, is_subsystem)
# is_subsystem=True: 有子命令的子系统（如 diagnosis）
# is_subsystem=False: 独立的顶层命令（如 precheck, version）
TOP_COMMANDS: List[Dict[str, Any]] = [
    {
        "name": "memory",
        "help": "内存快速排查：classify / memgraph / oom / javamem；可选 --deep-diagnosis 接深度诊断。",
        "is_subsystem": True,
    },
    {
        "name": "io",
        "help": "磁盘与 IO 专项：iofsstat、iodiagnose（与 service_name 对齐）。",
        "is_subsystem": True,
    },
    {
        "name": "net",
        "help": "网络专项：packetdrop、netjitter（与 service_name 对齐）。",
        "is_subsystem": True,
    },
    {
        "name": "load",
        "help": "系统负载与调度专项：delay、loadtask（与 service_name 对齐）。",
        "is_subsystem": True,
    },
    {
        "name": "precheck",
        "help": "环境预检查：验证阿里云认证配置和 SysOM API 权限（自动发现）。",
        "is_subsystem": False,
    },
    {
        "name": "configure",
        "help": "交互式将 RAM 用户 AK 写入 ~/.aliyun/config.json（跨 Shell 生效，推荐 Agent 环境）。",
        "is_subsystem": False,
    },
]


def build_parser() -> argparse.ArgumentParser:
    """构建顶层命令行解析器"""
    ap = argparse.ArgumentParser(
        prog="sysom_cli",
        description="Sysom CLI：统一命令行工具，支持多级命令自动发现。",
    )
    ap.add_argument(
        "--list-capabilities",
        action="store_true",
        help="列出所有子命令及支持的 local/remote/hybrid 模式",
    )
    ap.add_argument(
        "--json-errors",
        action="store_true",
        help="异常时 stdout 仍输出 JSON 信封",
    )
    top_sub = ap.add_subparsers(dest="top_cmd", metavar="TOPCMD")

    from sysom_cli.core.registry import CommandRegistry
    
    # 先发现顶层命令
    CommandRegistry.discover_commands(top_level=True)
    
    for top_spec in TOP_COMMANDS:
        name = top_spec["name"]
        is_subsystem = top_spec.get("is_subsystem", False)
        not_implemented = top_spec.get("not_implemented", False)
        
        p = top_sub.add_parser(name, help=top_spec["help"])
        
        if not_implemented:
            # 预留命令，不添加子命令
            continue
        
        if is_subsystem:
            # 子系统：子命令（如 memory classify、io iofsstat）
            CommandRegistry.discover_commands(subsystem=name)
            all_metadata = CommandRegistry.get_all_metadata(subsystem=name)
            
            if all_metadata:
                sub = p.add_subparsers(dest="sub_cmd", required=True, metavar="SUBCOMMAND")
                for cmd_name, metadata in all_metadata.items():
                    subp = sub.add_parser(cmd_name, help=metadata.get("help", ""))
                    for flags, kw in metadata.get("args", []):
                        fl = flags if isinstance(flags, (list, tuple)) else [flags]
                        subp.add_argument(*fl, **kw)
        else:
            # 顶层独立命令（如 precheck）
            # 从注册中心获取元数据并添加参数
            try:
                metadata = CommandRegistry.get_metadata(name)
                for flags, kw in metadata.get("args", []):
                    fl = flags if isinstance(flags, (list, tuple)) else [flags]
                    p.add_argument(*fl, **kw)
            except KeyError:
                # 命令未通过 @command_metadata 注册，可能是预留命令
                pass

    return ap


# ============================================================================
# 能力查询
# ============================================================================

# 能力注册表：由命令自动发现机制动态获取
def get_capabilities():
    """从注册中心获取所有命令的能力"""
    from sysom_cli.core.registry import CommandRegistry
    
    CommandRegistry.discover_commands(top_level=True)
    CommandRegistry.discover_commands()
    capabilities = {}
    
    for cmd_name in CommandRegistry.list_commands():
        metadata = CommandRegistry.get_metadata(cmd_name)
        supported_modes = metadata.get("supported_modes", {})
        subsystem = CommandRegistry._command_subsystem.get(cmd_name, "__top_level__")
        
        # 构建命令键：(subsystem, command_name) 或 (command_name,)
        if subsystem == "__top_level__":
            key = (cmd_name,)
        else:
            key = (subsystem, cmd_name)
        
        capabilities[key] = {
            "local": supported_modes.get("local", False),
            "remote": supported_modes.get("remote", False),
            "hybrid": supported_modes.get("hybrid", False),
        }
    
    return capabilities


def list_capabilities_json() -> str:
    import json
    
    capabilities = get_capabilities()
    out = []
    for key, modes in sorted(capabilities.items()):
        if len(key) == 1:
            # 顶层命令
            cmd_str = key[0]
            sub = key[0]
        else:
            # 子系统命令
            cmd_str = f"{key[0]} {key[1]}"
            sub = key[1]
        
        out.append({
            "command": cmd_str,
            "sub": sub,
            "local": modes["local"],
            "remote": modes["remote"],
            "hybrid": modes["hybrid"],
        })
    return json.dumps({"capabilities": out}, ensure_ascii=False, indent=2)


def list_capabilities_table() -> str:
    capabilities = get_capabilities()
    lines = ["command           local  remote hybrid", "------------------ ----- ------ ------"]
    for key, modes in sorted(capabilities.items()):
        if len(key) == 1:
            cmd = key[0]
        else:
            cmd = f"{key[0]} {key[1]}"
        lines.append(f"{cmd:<18} {str(modes['local']):<5} {str(modes['remote']):<6} {str(modes['hybrid'])}")
    return "\n".join(lines)


# ============================================================================
# 命令执行
# ============================================================================

def run_diagnosis(sub_cmd: str, ns: Namespace) -> dict:
    """执行 diagnosis 子命令（子系统，有多个子命令）"""
    from sysom_cli.core.executor import CommandExecutor
    from sysom_cli.lib.schema import error_envelope
    
    # 构建命令参数命名空间
    diag_ns = Namespace(
        **{k: v for k, v in vars(ns).items() if k not in ("top_cmd", "sub_cmd", "rest")}
    )
    diag_ns.cmd = sub_cmd
    
    try:
        # 使用统一执行器
        return CommandExecutor.execute(sub_cmd, diag_ns)
    except KeyError as e:
        return error_envelope(
            sub_cmd,
            "command_not_found",
            f"命令 '{sub_cmd}' 未找到: {e}",
        )
    except Exception as e:
        return error_envelope(
            sub_cmd,
            "execution_error",
            f"执行失败: {e}",
        )


def run_top_level_command(cmd_name: str, ns: Namespace) -> dict:
    """执行顶层独立命令（如 precheck）"""
    from sysom_cli.core.executor import CommandExecutor
    from sysom_cli.lib.schema import error_envelope
    
    try:
        # 使用统一执行器
        return CommandExecutor.execute(cmd_name, ns)
    except KeyError as e:
        return error_envelope(
            cmd_name,
            "command_not_found",
            f"命令 '{cmd_name}' 未找到: {e}",
        )
    except Exception as e:
        return error_envelope(
            cmd_name,
            "execution_error",
            f"执行失败: {e}",
        )


# ============================================================================
# 主入口
# ============================================================================

def main() -> int:
    ap = build_parser()
    ns = ap.parse_args()

    if getattr(ns, "list_capabilities", False):
        print(list_capabilities_json())
        return 0

    top_cmd = getattr(ns, "top_cmd", None)
    if not top_cmd:
        ap.print_help()
        return 0

    # 查找命令配置
    cmd_spec = next((c for c in TOP_COMMANDS if c["name"] == top_cmd), None)
    
    if not cmd_spec:
        print(f'{{"ok": false, "error": "未知命令: {top_cmd}"}}')
        return 1
    
    # 检查是否是未实现的预留命令
    if cmd_spec.get("not_implemented"):
        from sysom_cli.lib.schema import FORMAT_NAME

        print(
            '{"format":"'
            + FORMAT_NAME
            + '","ok":false,"error":{"code":"not_implemented","message":"该命令预留，尚未实现"}}'
        )
        return 1
    
    from sysom_cli.lib.schema import dumps
    
    if cmd_spec.get("is_subsystem"):
        # 子系统命令：需要子命令（如 memory classify、io iofsstat）
        sub_cmd = getattr(ns, "sub_cmd", None)
        if not sub_cmd:
            ap.print_help()
            return 0
        
        out = run_diagnosis(sub_cmd, ns)
        print(dumps(out))
        return 0 if out.get("ok") else 1
    else:
        # 顶层独立命令（如 precheck）
        result = run_top_level_command(top_cmd, ns)
        
        # 统一输出 JSON
        print(dumps(result))
        return 0 if result.get("ok") else 1


if __name__ == "__main__":
    raise SystemExit(main())
