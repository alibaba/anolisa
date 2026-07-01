# Copilot Shell（cosh）

Copilot Shell（cosh）是基于上游 Qwen Code v0.9.0 构建的 AI 增强交互式终端助手。它提供自然语言操作系统的接口，同时保持完整的 Shell 兼容性。

---

## 概述

cosh 将传统 Shell 体验与 AI 能力结合：

- **AI 增强 Shell** — 自然语言命令与标准 Shell 并用
- **Hook 系统** — 可扩展的 PreToolUse 事件 hook，集成 ANOLISA 组件（tokenless、agentsight）
- **工具审批** — 执行潜在危险命令前的交互式审批流程
- **上下文感知** — 理解工作目录、git 状态和环境信息

**可执行文件：** `cosh`、`copilot`、`co`（均指向同一入口点）

---

## 安装

### 方式一：anolisa CLI（推荐）

```bash
anolisa install cosh
```

### 方式二：YUM（Alinux，需配置 ANOLISA YUM 源）

```bash
sudo yum install copilot-shell
```

### 方式三：源码编译（开发者）

```bash
cd src/copilot-shell && make build
```

---

## 快速开始

启动 cosh：

```bash
cosh
```

进入 cosh 后，可以：

```bash
# 使用标准 Shell 命令
ls -la
git status

# 使用自然语言
> "查找最近一周修改过的所有 Python 文件"

# 混合使用
> "压缩 /var/log 下超过 30 天的所有 *.log 文件"
```

---

## 功能

### 工具审批

当 cosh 识别到可能有风险的命令时，会提示确认：

```
⚠️  即将执行以下命令：
    rm -rf ./build/
    
  [Y] 批准  [N] 拒绝  [E] 编辑
```

### Hook 系统

cosh 支持拦截 PreToolUse 事件的 hook。ANOLISA 组件通过 hook 集成——例如 Tokenless extension 在工具调用到达模型前拦截并压缩 Schema。

### Skills

cosh 支持 skill 系统以扩展 Agent 能力：

| 路径 | 作用域 |
|------|--------|
| `.copilot-shell/skills/` | 项目级 skill |
| `~/.copilot-shell/skills/` | 用户级 skill |
| `/usr/share/anolisa/skills/` | 系统级 skill |

### 快捷键

| 按键 | 操作 |
|------|------|
| `Ctrl+L` | 清屏 |
| `Ctrl+C` | 取消当前输入 |
| `Tab` | 自动补全 |
| `↑/↓` | 历史导航 |

---

## 配置

配置通过运行时设置和环境变量管理。Skills、hooks 和 extensions 存放于 `~/.copilot-shell/` 目录下。

```
~/.copilot-shell/
├── skills/           # 用户级 skills
├── extensions/       # Extensions（如 tokenless）
└── ...
```

---

## 参见

- [anolisa CLI](anolisa-cli.md)
- [Tokenless 集成](../token-saving/tokenless.md)
