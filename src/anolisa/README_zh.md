# anolisa

[English](README.md)

ANOLISA 统一 CLI 网关——管理组件生命周期、框架适配器、OS 基础层和系统服务。anolisa 是 [ANOLISA](../../README_zh.md) 项目的主用户入口，通过一条命令完成所有组件的安装、更新、诊断和编排。

## 快速开始

```bash
# 列出可用组件
anolisa list

# 安装组件
anolisa install agent-memory

# 检查组件健康
anolisa status agent-memory

# 更新所有
anolisa update all
```

## 命令概览

### 一级命令 — 组件生命周期

| 命令 | 说明 |
|------|------|
| `list` | 列出可用组件（别名：`ls`） |
| `install` | 从配置的后端安装组件（raw / rpm） |
| `uninstall` | 卸载组件 |
| `update` | 更新组件、CLI 自身（`self`）或全部（`all`） |
| `status` | 显示组件健康状态 |
| `doctor` | 诊断问题并给出修复建议 |
| `logs` | 查询组件日志 |
| `restart` | 重启组件服务 |

### 二级命令 — 管理

| 命令 | 说明 |
|------|------|
| `adapter` | 管理组件到框架的适配器 |
| `osbase kernel` | 内核模块与 eBPF 管理 |
| `osbase sandbox` | 沙箱运行时管理（runc、gvisor、firecracker 等） |
| `osbase security` | 安全覆盖层管理 |
| `system` | 系统辅助守护进程生命周期 |
| `register` | 加入/离开 Agentic OS Co-Build Program |
| `env` | 显示环境检测结果 |
| `bug` | 生成 bug 报告 |

## 架构

五 crate Cargo workspace：

| Crate | 职责 |
|-------|------|
| `anolisa-cli` | 命令解析、分发、终端 UI |
| `anolisa-core` | 组件解析、适配器管理、osbase 安装逻辑 |
| `anolisa-env` | 环境检测（发行版、架构、能力） |
| `anolisa-build` | 构建时代码生成与资源嵌入 |
| `anolisa-platform` | 文件系统布局、systemd 集成、IPC、权限辅助 |

支持双后端：**raw**（OSS tar.gz）和 **rpm**（dnf 仓库）。

## 许可证

Apache License 2.0 — 详见 [LICENSE](../../LICENSE)。
