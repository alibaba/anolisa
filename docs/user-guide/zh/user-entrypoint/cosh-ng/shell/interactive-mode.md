# 交互模式

cosh-shell 在原生 PTY 之上运行 bash/zsh，通过 OSC 转义序列标记命令边界，实现 AI 分析与工具审批的无缝集成。

## PTY 主机

cosh-shell 通过 `openpty()` 创建伪终端对，在 slave 端启动 bash 或 zsh 子进程：

```
┌──────────────────────┐
│    cosh-shell        │
│  ┌────────────────┐  │       ┌──────────────┐
│  │  PTY master    │──────────│  bash/zsh    │
│  └────────────────┘  │       │  (PTY slave) │
│  ┌────────────────┐  │       └──────────────┘
│  │  OSC Parser    │  │
│  └────────────────┘  │
└──────────────────────┘
```

### Shell 选择

```bash
cosh-shell                    # 默认使用 auto（自动检测）
cosh-shell --shell zsh        # 使用 zsh
cosh-shell raw qwen --shell zsh
cosh-shell --resume           # Shell 启动后打开会话选择器
cosh-shell --resume <id>      # 选择要恢复的 provider 对话
```

Shell 选择优先级：
1. `--shell` 参数
2. `COSH_SHELL_RAW_SHELL` 环境变量
3. 配置文件 `shell.default`
4. 自动检测（默认 bash）

### 运行模式

| 模式 | 说明 |
|------|------|
| 默认（无子命令） | 使用配置的适配器启动交互式 shell |
| `raw [adapter]` | 显式指定适配器 |
| `-c <command>` | 执行命令后退出（直通底层 shell） |
| `-- <command>` | 直接执行命令后退出 |
| `--isolated` | 隔离模式，不加载用户 rcfile |
| `--login` / `-l` | 作为登录 shell 启动 |

## OSC 标记系统

cosh-shell 注入自定义 rcfile（bashrc/zshrc）到子 shell，通过 OSC 1337 转义序列标记命令生命周期：

```
ESC]1337;COSH;<payload>BEL
```

标记的事件包括：
- Shell 就绪（prompt 显示）
- 命令开始执行（preexec）
- 命令执行结束（precmd，携带退出码）
- 工作目录变更

这些标记使 cosh-shell 精确识别：
1. 用户何时输入命令
2. 命令何时开始/结束
3. 命令的退出码
4. 当前工作目录

## 输入分类

用户在 shell 中的输入被分类为以下类型：

| 类型 | 说明 | 示例 |
|------|------|------|
| Shell 命令 | 普通 shell 命令 | `ls -la`、`git status` |
| Slash 命令 | `/` 开头的内置控制命令 | `/help`、`/mode` |
| Natural Language | 自然语言提问 | 由 AI 适配器处理 |
| Agent Marker | Agent 执行标记 | 内部使用 |

## Slash 命令

| 命令 | 说明 |
|------|------|
| `/help` | 显示帮助信息 |
| `/mode [approval\|analysis] [value]` | 查看或切换模式 |
| `/config [key] [value]` | 查看或修改运行时配置 |
| `/hooks [list\|enable\|disable] [name]` | 管理钩子 |
| `/extensions [list\|enable\|disable] [name]` | 管理扩展 |
| `/skills [list\|enable\|disable] [name]` | 管理技能 |
| `/session [status\|list\|resume <id>\|clear ...]` | 管理 provider 对话 |
| `/resume [id]` | 会话选择器或会话选择的别名 |
| `/debug [state\|events\|adapter]` | 调试信息 |
| `/auth` | 触发认证流程 |

选择器按键、工作空间作用域、清理确认和可恢复错误处理详见
[会话恢复](session-recovery.md)。

## 启动流程

1. 解析命令行参数（shell 类型、适配器、模式）
2. 安装终端恢复处理（SIGTERM/SIGHUP/panic 时恢复 termios）
3. 加载配置（`~/.copilot-shell/config.toml`）
4. 初始化日志（文件输出到 `~/.copilot-shell/logs/`）
5. 创建 PTY 会话，启动 bash/zsh 子进程
6. 注入 OSC 标记脚本
7. 启动 AI 适配器连接
8. 渲染启动横幅（COSH logo + 适配器/shell/审批模式信息）
9. 进入主事件循环

## 终端恢复

cosh-shell 在以下场景自动恢复终端状态（termios）：

- 进程收到 SIGTERM / SIGHUP / SIGQUIT
- panic 触发
- 正常退出

确保异常退出时终端不会留在 raw mode。

## Native 模式 vs 隔离模式

| 特性 | Native 模式（默认） | 隔离模式（`--isolated`） |
|------|---------------------|--------------------------|
| 用户 rcfile | 加载（~/.bashrc 等） | 跳过 |
| PS1 提示符 | 保留用户设置 | 使用 `cosh-osc$ ` |
| 历史记录 | 加载 $HISTFILE | 不加载 |
| 环境变量 | 继承 | 最小化 |

## 会话工作目录

cosh-shell 在 `~/.copilot-shell/` 下为每个会话创建工作目录，存储：

- OSC 标记脚本
- 命令输出引用（24 小时自动清理）
- 终端恢复请求文件
- Shell handoff 请求文件
