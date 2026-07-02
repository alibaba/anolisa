# AI 分析

cosh-shell 在检测到命令失败时，可自动或按需调用 AI 适配器分析失败原因并给出建议。

## 分析模式

通过 `/mode analysis <mode>` 或配置 `shell.analysis_mode` 切换：

| 模式 | 说明 |
|------|------|
| `smart` | 智能模式：严重错误显示操作卡片，一般错误提示可分析 |
| `auto` | 自动模式：检测到失败立即自动分析 |
| `manual` | 手动模式：显示操作卡片，等待用户确认后分析 |

## 失败分类

cosh-shell 对命令退出码和输出进行语义分析，将失败归类：

| 分类 | 示例 | 说明 |
|------|------|------|
| `CommandNotFound` | `command not found` | 命令不存在 |
| `PermissionDenied` | `Permission denied` | 权限不足 |
| `BuildOrTestFailure` | `error[E0308]` | 编译/测试错误 |
| `AbnormalSignal` | SIGSEGV | 异常信号终止 |
| `GenericRuntimeFailure` | 非零退出码 | 一般运行时错误 |
| `UsageOrHelp` | `Usage:` 输出 | 用法错误 |
| `UnknownFailure` | 其他 | 未分类失败 |

以下分类视为"非真正失败"，不触发分析：
- `Success` — 实际成功
- `InteractiveCancel` — 用户主动中断
- `UserInterrupt` — Ctrl+C
- `PipelineNormal` — 管道正常退出码
- `ProviderOrInternalArtifact` — 内部工具产生的退出码

## 分析处置矩阵

| 失败分类 | Auto 模式 | Smart 模式 | Manual 模式 |
|----------|-----------|------------|-------------|
| CommandNotFound / PermissionDenied / AbnormalSignal / BuildOrTestFailure | 自动分析 | 操作卡片 | 操作卡片 |
| GenericRuntimeFailure | 自动分析 | 提示 | 操作卡片 |
| UnknownFailure | 操作卡片 | 提示 | 提示 |
| UsageOrHelp | 提示 | 静默 | 静默 |

处置类型说明：
- **自动分析** — 立即调用 AI 适配器分析
- **操作卡片** — 渲染交互卡片，用户可选择"分析"或"跳过"
- **提示** — 显示简短提示，用户可输入 slash 命令触发分析
- **静默** — 仅记录，不干扰用户

## 分析流程

```
命令执行失败（exit code ≠ 0）
       │
       ▼
  失败语义分类
       │
       ▼
  处置决策（按分析模式）
       │
       ├── AutoAnalyze → 直接启动 Agent 分析
       ├── ActionCard  → 渲染操作卡片 → 等待用户确认
       ├── Hint        → 显示简短提示
       └── SilentRecord → 静默记录
```

## Agent 分析过程

1. 收集失败上下文：命令文本、退出码、输出摘录（最多 8KB）
2. 构造 prompt 发送到 AI 适配器（cosh-core）
3. 适配器流式返回分析结果
4. cosh-shell 以 Markdown 格式渲染分析内容
5. 用户可在分析过程中 Ctrl+C 取消

## 配置

```toml
[shell]
# 分析模式：smart | auto | manual
analysis_mode = "smart"
```

运行时切换：

```
/mode analysis smart
/mode analysis auto
/mode analysis manual
```
