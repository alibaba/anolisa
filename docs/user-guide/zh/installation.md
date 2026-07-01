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

| 组件 | 说明 | 模式 |
|------|------|------|
| `cosh` | Copilot Shell — AI 终端助手 | user |
| `os-skills` | 系统管理与 DevOps 技能 | user |
| `tokenless` | Token 优化（压缩） | user |
| `ws-ckpt` | 工作区快照/回滚 | user |
| `skillfs` | FUSE 虚拟技能文件系统 | user |
| `agent-memory` | 基于 MCP 的持久化记忆 | user |
| `agentsight` | eBPF 追踪与 Dashboard | **system** |
| `agent-sec-core` | 安全加固 | **system** |

> **注意**：标记为 **system** 的组件需要 `sudo`：
> ```bash
> sudo anolisa install agentsight
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

适配器将组件桥接到特定 Agent 框架。安装组件后再安装适配器：

```bash
anolisa adapter install <component> --runtime <runtime>
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

移除所有 ANOLISA 组件：

```bash
anolisa uninstall --all
```

---

## 升级

更新特定组件：

```bash
anolisa update <component>
```

更新所有已安装组件：

```bash
anolisa update --all
```

---

## 下一步

- [anolisa CLI 参考](user-entrypoint/anolisa-cli.md)
- [Copilot Shell](user-entrypoint/copilot-shell.md)
- [故障排查](troubleshooting.md)
