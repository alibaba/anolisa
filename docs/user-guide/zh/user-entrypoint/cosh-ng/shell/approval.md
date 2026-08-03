# 工具审批

[English](../../../../en/user-entrypoint/cosh-ng/shell/approval.md)

Agent 要执行受保护的操作时，cosh 会先显示审批卡片。卡片会告诉你将要
调用哪个工具、传入什么内容，以及哪些风险或 Hook 结果需要注意。

## 选择审批模式

用 `/mode approval <mode>` 切换，也可以设置 `shell.approval_mode`。

| 模式 | 含义 | 对 cosh-core 的映射 |
|------|------|---------------------|
| `recommend` | 只读工作可直接执行；修改状态或访问外部系统时请求确认 | `strict` |
| `auto` | 默认模式；符合条件的低风险工具和 FileEdit 可直接执行，Shell、网络、MCP 和外部工具仍会询问 | `auto` |
| `trust` | 显式确认后，常规调用不再显示卡片 | `trust` |

切换到 trust 模式需要二次确认。

```
/mode approval trust confirm
```

Trust 仍受安全门禁约束。Reboot、shutdown、halt 等无法恢复的系统控制命令始终
需要明确审批，即使命令经过 wrapper 包装或已预先信任。高风险卡片不会创建持久
trust key。

## 读懂审批卡片

工具需要审批时，cosh-shell 会在终端内显示审批面板。

```
┌─────────────────────────────────────────┐
│ 🔧 Tool: shell                    [1/3] │
│ Risk: medium                            │
│─────────────────────────────────────────│
│ Command:                                │
│   rm -rf /tmp/old-build                 │
│─────────────────────────────────────────│
│ ⚠ Hook: sandbox-guard                   │
│   "命令匹配风险模式"                       │
│─────────────────────────────────────────│
│ [✓ Approve]  [ Deny ]  [ Details ]      │
└─────────────────────────────────────────┘
```

决定前先核对工具名称、输入预览、风险和 Hook 警告。预览被截断时，用 Details
展开完整内容。多个请求正在排队时，右上角的数字会显示当前位置。

## 执行已批准的 Shell 命令

批准 `shell` 工具后，cosh 会把命令交给前台 Shell。

```
用户批准 shell 命令
       │
       ▼
cosh-shell 将命令注入 PTY
       │
       ▼
bash/zsh 在前台执行（用户可交互）
       │
       ▼
执行结果通过 OSC 标记回传
```

这条命令的行为与你亲自输入时一样。

- 命令输出直接显示在终端
- 用户可以实时交互（如确认提示）
- Ctrl+C 可中断执行

已批准的 Shell 命令会串行执行。命令等待终端输入时，cosh 可以显示最近的提示内容。
密码、pager 或普通 stdin 等待默认在 120 秒后中断。设置
`shell.input_wait_timeout_secs = 0` 可以关闭该超时。全屏 TUI 和管道读取不会计时。

## 查看历史审批

cosh 会在运行日志中记录审批结果。启用审计日志后，脱敏副本也会写入持久审计时间线。
需要追查之前的操作时，查看[审计指南](../cli/audit.md)。

## 配置

```toml
[shell]
# 审批模式 recommend | auto | trust
approval_mode = "auto"

# 信任的命令列表
trusted_commands = ["ls", "cat", "echo"]

# 已批准前台命令的输入等待超时；0 表示禁用
input_wait_timeout_secs = 120
```

`trusted_commands` 使用精确的 trust key，不按 Shell 字符串片段匹配，也无法越过
无法恢复命令的安全门禁。环境变量覆盖见[配置](../configuration.md)。
