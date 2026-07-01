# anolisa CLI

`anolisa` CLI 是管理所有 ANOLISA 组件的统一命令行界面。它负责安装、更新、状态监控和诊断。

---

## 安装

### 方式 A：安装脚本（推荐）

```bash
curl -fsSL https://agentic-os.sh | sh
```

### 方式 B：YUM（Alinux）

```bash
sudo yum install anolisa
```

验证安装：

```bash
anolisa --version
```

---

## 命令

### install

安装组件：

```bash
anolisa install <component>

# system mode 安装（需要 root）
sudo anolisa install <component>

# 安装全部组件
anolisa install --all
```

### uninstall

移除组件：

```bash
anolisa uninstall <component>

# 移除全部组件
anolisa uninstall --all
```

### update

更新已安装组件：

```bash
anolisa update <component>

# 更新全部
anolisa update --all
```

### status

显示组件安装状态：

```bash
anolisa status

# 查看特定组件状态
anolisa status <component>
```

### list

列出所有可用组件：

```bash
anolisa list
```

### env

显示环境信息和系统能力：

```bash
anolisa env
```

### doctor

对已安装组件运行健康检查：

```bash
anolisa doctor

# 检查特定类别
anolisa doctor --check <category>
```

类别：`network`、`build-deps`、`ebpf`、`fuse`、`btrfs`

### adapter

管理组件适配器：

```bash
# 发现适配器
anolisa adapter scan

# 启用适配器
anolisa adapter enable <component> [framework]

# 禁用适配器
anolisa adapter disable <component> [framework]

# 查看适配器状态
anolisa adapter status [component]
```

### logs

查看组件日志：

```bash
anolisa logs <component>
anolisa logs <component> --follow
anolisa logs <component> --tail 50
```

### bug

生成诊断报告：

```bash
anolisa bug
```

---

## 全局选项

| 选项 | 说明 |
|------|------|
| `--verbose` | 启用详细输出 |
| `--quiet` | 仅显示错误输出 |
| `--version` | 显示 CLI 版本 |
| `--help` | 显示帮助信息 |

---

## 示例

```bash
# 完整安装流程
curl -fsSL https://agentic-os.sh | sh
anolisa env
anolisa install cosh
anolisa install tokenless
anolisa adapter enable tokenless cosh
anolisa doctor
anolisa status
```

---

## 配置

CLI 配置文件：`~/.config/anolisa/config.toml`

```toml
[registry]
# 组件注册表 URL
url = "https://registry.agentic-os.sh"

[install]
# 默认安装模式："user" 或 "system"
mode = "user"

# user mode 安装前缀
prefix = "~/.local"
```

---

## 参见

- [安装指南](../installation.md)
- [故障排查](../troubleshooting.md)
