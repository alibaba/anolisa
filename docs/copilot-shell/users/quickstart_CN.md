# 快速入门

> 👏 欢迎使用 Copilot Shell！

这份快速入门指南将让您在几分钟内开始使用 AI 驱动的编程和系统管理。在结束时，您将了解如何使用 Copilot Shell 进行常见的开发和运维任务。

## 开始之前

请确保您具备：

- 一台阿里云 Linux (Alinux) 机器上的**终端**
- 一个要管理的代码项目或系统
- 已配置一种受支持的身份验证方法（参见下面的[身份验证](#步骤-2-身份验证)）

## 步骤 1：安装 Copilot Shell

### RPM（推荐）

```bash
sudo yum install copilot-shell
```

### 从源码构建

需要 [Node.js 20+](https://nodejs.org/download)。您可以使用 `node -v` 检查您的版本。

```bash
cd src/copilot-shell
make build
```

构建成功后，打包的二进制文件可在 `dist/cli.js` 找到。

## 步骤 2：身份验证

首次启动 Copilot Shell 时，您需要配置身份验证：

```bash
cosh
```

在会话内使用 `/auth` 命令选择您的提供商：

```bash
/auth
```

### 支持的提供商

| 提供商 | 命令 | 描述 |
|--------|------|------|
| Qwen OAuth | `cosh` | 免费套餐，每天 2,000 次请求 —— 按屏幕提示操作 |
| API 密钥 | `cosh --auth apikey` | Qwen 模型的直接 API 密钥 |
| 自定义提供商 | `cosh --auth openai` | 任何 OpenAI 兼容端点 —— DashScope、DeepSeek、Kimi、GLM、MiniMax 或您自己的 |

> [!tip]
>
> 要在以后切换账户或提供商，请在 Copilot Shell 内使用 `/auth` 命令。

## 步骤 3：开始您的第一个会话

在任何项目目录中打开终端并启动 Copilot Shell：

```bash
cd /path/to/your/project
cosh
```

您将看到带有会话信息和最近对话的欢迎界面。键入 `/help` 查看可用命令。

> [!note]
>
> 您也可以使用别名 `co` 或 `copilot` 替代 `cosh`。

## 与 Copilot Shell 交谈

### 问您的第一个问题

Copilot Shell 将分析您的文件并提供答案。您可以询问有关代码库的问题：

```
解释文件夹结构
```

或者询问系统状态：

```
显示当前磁盘使用情况和内存消耗最高的进程
```

> [!note]
>
> Copilot Shell 会在需要时读取您的文件 —— 您无需手动添加上下文。它还可以访问用于系统管理任务的 OS 级技能。

### 进行第一次代码更改

尝试一个简单的编码任务：

```
在主文件中添加一个 hello world 函数
```

Copilot Shell 将：

1. 找到合适的文件
2. 向您显示建议的更改
3. 征求您的批准
4. 进行编辑

> [!note]
>
> Copilot Shell 在修改文件之前总是征求许可。您可以批准单个更改或为会话启用"全部接受"模式。

### 系统管理

Copilot Shell 与 OS 级技能集成，用于常见运维任务：

```
检查是否有任何失败的 systemd 服务
```

```
分析 nginx 访问日志，找出过去一小时内的前 10 个 IP
```

```
设置一个 cron 作业，每天凌晨 3 点清理 /tmp
```

### 在 Copilot Shell 中使用 Git

Git 操作变得会话化：

```
我更改了哪些文件？
```

```
用描述性消息提交我的更改
```

```
创建一个名为 feature/quickstart 的新分支
```

```
帮我解决合并冲突
```

### 修复错误或添加功能

用自然语言描述您想要的内容：

```
为用户注册表单添加输入验证
```

或者修复现有问题：

```
有一个错误，用户可以提交空表单 - 修复它
```

Copilot Shell 将：

- 定位相关代码
- 理解上下文
- 实施解决方案
- 如果可用则运行测试

### 进入交互式 Shell

使用 `/bash` 命令从 Copilot Shell 内部进入交互式 Shell：

```
/bash
```

键入 `exit` 返回 Copilot Shell 会话。

### 其他常见工作流程

**重构代码**

```
重构认证模块，使用 async/await 而不是回调
```

**编写测试**

```
为计算器函数编写单元测试
```

**更新文档**

```
使用安装说明更新 README
```

**代码审查**

```
审查我的更改并提出改进建议
```

> [!tip]
>
> **记住**：Copilot Shell 是您的 AI 结对程序员和系统管理员助手。像对待一位乐于助人的同事一样与它交谈 —— 描述您想要实现的目标，它将帮助您达成目标。

## 基本命令

以下是日常使用的最重要命令：

| 命令 | 作用 | 示例 |
|------|------|------|
| `cosh` | 启动 Copilot Shell | `cosh` |
| `/auth` | 更改身份验证方法 | `/auth` |
| `/help` | 显示可用命令的帮助 | `/help` 或 `/?` |
| `/bash` | 进入交互式 Shell | `/bash` |
| `/model` | 在配置的模型之间切换 | `/model` |
| `/compress` | 用摘要替换聊天历史以节省令牌 | `/compress` |
| `/clear` | 清除终端屏幕 | `/clear`（快捷键：`Ctrl+L`） |
| `/theme` | 更改视觉主题 | `/theme` |
| `/language` | 查看或更改语言设置 | `/language` |
| → `ui [lang]` | 设置 UI 界面语言 | `/language ui zh-CN` |
| → `output [lang]` | 设置 LLM 输出语言 | `/language output Chinese` |
| `/quit` | 退出 Copilot Shell | `/quit` 或 `/exit` |

## 初学者专业提示

**对您的请求要具体**

- 不要说："修复错误"
- 尝试说："修复登录错误，用户输入错误凭据后看到空白屏幕"

**使用逐步指令**

- 将复杂任务分解为步骤：

```
1. 为用户资料创建一个新的数据库表
2. 创建一个 API 端点来获取和更新用户资料
3. 构建一个网页，允许用户查看和编辑他们的信息
```

**让 Copilot Shell 先探索**

- 在进行更改之前，让它了解您的代码：

```
分析数据库架构
```

**使用快捷方式节省时间**

- 按 `?` 查看所有可用键盘快捷方式
- 使用 Tab 进行命令补全
- 按 ↑ 查看命令历史
- 键入 `/` 查看所有斜杠命令

## 获取帮助

- **在 Copilot Shell 中**：键入 `/help` 或询问"我如何..."
- **文档**：浏览[用户指南](overview.md)
- **问题**：在项目存储库中提交问题