# 安装指南

本指南涵盖 ANOLISA 的渐进式安装——从 CLI 工具到各组件及适配器的配置。

---

## 第一步：安装 ANOLISA CLI

`anolisa` CLI 是管理所有 ANOLISA 组件的统一入口。

### 方式 A：安装脚本（推荐）

```bash
curl -fsSL https://agentic-os.sh | sh
```

### 方式 B：YUM（Alinux）

```bash
sudo yum install anolisa
```

安装后验证：

```bash
anolisa --version
```

---

## 第二步：环境检测

运行环境检查以识别系统能力：

```bash
anolisa env
```

将显示：
- 操作系统和架构
- 可用文件系统（btrfs 用于 ws-ckpt）
- FUSE 可用性（用于 skillfs）
- 已安装的 Agent 运行时（cosh、OpenClaw、Hermes）
- 内核特性（eBPF 用于 agentsight）

---

## 第三步：安装组件

根据需要逐个安装组件：

```bash
anolisa install <component>
```

### 可用组件

| 组件 | 说明 | 支持的模式 |
|------|------|------------|
| `cosh` | Copilot Shell — AI 终端助手 | user、system |
| `os-skills` | 系统管理与 DevOps 技能 | user、system |
| `tokenless` | Token 优化（压缩） | user、system |
| `ws-ckpt` | 工作区快照/回滚 | **system** |
| `skillfs` | FUSE 虚拟技能文件系统 | **system** |
| `agent-memory` | 基于 MCP 的持久化记忆 | user、system |
| `agentsight` | eBPF 追踪与 Dashboard | **system** |
| `agent-sec-core` | 安全加固 | **system** |

> **注意**：仅支持 system mode 的组件需要 `sudo` 并显式选择 system scope：
> ```bash
> sudo anolisa --install-mode system install agentsight
> ```

### 安装全部组件

```bash
anolisa install --all
```

### YUM 替代方式（Alinux）

每个组件也可通过 YUM 安装：

```bash
sudo yum install <component>
```

---

## 第四步：适配器配置

适配器将组件桥接到特定 Agent 框架。安装组件后再启用适配器：

```bash
anolisa adapter scan
anolisa adapter enable <component> [framework]
```

### 示例

```bash
# Tokenless cosh hook
/usr/share/tokenless/scripts/install.sh --cosh

# Tokenless OpenClaw 插件
/usr/share/tokenless/scripts/install.sh --openclaw

# ws-ckpt OpenClaw 插件
ws-ckpt plugin install --runtime openclaw

# ws-ckpt Hermes 插件
ws-ckpt plugin install --runtime hermes
```

---

## 第五步：验证安装

查看所有已安装组件的状态：

```bash
anolisa status
```

运行内置诊断工具：

```bash
anolisa doctor
```

---

## 卸载

移除特定组件：

```bash
anolisa uninstall <component>
```

当前没有批量卸载命令。先列出安装记录，再逐个卸载目标组件，以便分别确认其
authority 和系统软件包移除策略：

```bash
anolisa list --installed
anolisa uninstall <component>
```

---

## 升级

更新特定组件：

```bash
anolisa update <component>
```

更新所有已安装组件：

```bash
anolisa update all
```

`update all` 只更新已记录的组件，不更新 CLI；更新 CLI 请使用
`anolisa update self`。

---

## 下一步

- [anolisa CLI 参考](user-entrypoint/anolisa-cli.md)
- [Copilot Shell](user-entrypoint/copilot-shell/QUICKSTART.md)
- [故障排查](troubleshooting.md)
