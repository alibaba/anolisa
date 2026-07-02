# 工具审批

cosh-shell 的工具审批系统在 AI 适配器请求执行工具时，以可视化卡片呈现操作详情，让用户决定是否允许。

## 审批模式

通过 `/mode approval <mode>` 或配置 `shell.approval_mode` 切换：

| 模式 | 含义 | 对 cosh-core 的映射 |
|------|------|---------------------|
| `recommend` | 推荐模式：所有工具调用需审批 | `strict` |
| `auto` | 自动模式（默认）：仅 shell 命令需审批 | `auto` |
| `trust` | 信任模式：所有工具自动执行（需 `confirm` 确认） | `trust` |

切换到 trust 模式需要二次确认：

```
/mode approval trust confirm
```

## 审批卡片

当工具需要审批时，cosh-shell 渲染内联审批面板：

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

### 卡片元素

| 元素 | 说明 |
|------|------|
| Tool | 工具名称 |
| Risk | 风险等级（由钩子评估） |
| Queue | 队列位置（当多个请求排队时） |
| Command/Input | 工具输入预览 |
| Hook warnings | 钩子产生的警告信息 |
| Actions | 可选操作按钮 |

### 用户操作

| 操作 | 说明 |
|------|------|
| Approve | 允许执行 |
| Deny | 拒绝执行 |
| Details | 展开完整输入内容 |

## Shell 命令 Handoff

当审批的工具是 `shell` 类型且用户批准时，命令会"移交"到前台 PTY 执行（而非由 cosh-core 在后台执行）：

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

这意味着：
- 命令输出直接显示在终端
- 用户可以实时交互（如确认提示）
- Ctrl+C 可中断执行

## 审批日志

所有审批决策记录在内存中的 journal：

| 字段 | 说明 |
|------|------|
| `id` | 审批请求唯一标识 |
| `run_id` | 所属 Agent 运行 ID |
| `kind` | 请求类型（Tool / ShellCommand） |
| `risk` | 风险等级 |
| `decision` | 最终决策（Allow / Deny / Cancel） |
| `subject` | 工具名称 |
| `preview` | 操作预览 |

## 与 cosh-core 审批协议的关系

cosh-shell 的审批系统是 cosh-core JSONL 协议中 `can_use_tool` 控制请求的前端实现：

```
Core → Shell:  {"type":"control_request","request_id":"apr-1","request":{"subtype":"can_use_tool",...}}
                       │
                       ▼
              cosh-shell 渲染审批卡片
                       │
                       ▼ (用户决策)
Shell → Core:  {"type":"control_response","response":{"subtype":"tool_approval","request_id":"apr-1","response":{"behavior":"allow"}}}
```

## 配置

```toml
[shell]
# 审批模式：recommend | auto | trust
approval_mode = "auto"

# 信任的命令列表（始终自动审批）
trusted_commands = ["ls", "cat", "echo"]
```
