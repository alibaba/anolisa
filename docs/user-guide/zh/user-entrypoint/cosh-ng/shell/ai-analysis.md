# AI 分析

[English](../../../../en/user-entrypoint/cosh-ng/shell/ai-analysis.md)

cosh-shell 在检测到命令失败时，可自动或按需调用 AI 适配器分析失败原因并给出建议。

## 分析模式

通过 `/mode analysis <mode>` 或配置 `shell.analysis_mode` 切换。

| 模式 | 说明 |
|------|------|
| `smart` | 严重错误显示操作卡片，一般错误提示可以分析 |
| `auto` | 检测到失败后立即自动分析 |
| `manual` | 显示操作卡片，等待用户确认后分析 |

## 失败分类

cosh-shell 根据命令退出码和输出判断失败类型。

| 分类 | 示例 | 说明 |
|------|------|------|
| `CommandNotFound` | `command not found` | 命令不存在 |
| `PermissionDenied` | `Permission denied` | 权限不足 |
| `BuildOrTestFailure` | `error[E0308]` | 编译/测试错误 |
| `AbnormalSignal` | SIGSEGV | 异常信号终止 |
| `GenericRuntimeFailure` | 非零退出码 | 一般运行时错误 |
| `UsageOrHelp` | `Usage:` 输出 | 用法错误 |
| `UnknownFailure` | 其他 | 未分类失败 |

以下情况不会触发分析。

- `Success` 表示实际成功
- `InteractiveCancel` 表示用户主动取消
- `UserInterrupt` 表示用户按下 Ctrl+C
- `PipelineNormal` 表示管道正常退出
- `ProviderOrInternalArtifact` 表示模型服务或内部工具产生的退出码

## 分析处置矩阵

| 失败分类 | Auto 模式 | Smart 模式 | Manual 模式 |
|----------|-----------|------------|-------------|
| CommandNotFound / PermissionDenied / AbnormalSignal / BuildOrTestFailure | 自动分析 | 操作卡片 | 操作卡片 |
| GenericRuntimeFailure | 自动分析 | 提示 | 操作卡片 |
| UnknownFailure | 操作卡片 | 提示 | 提示 |
| UsageOrHelp | 提示 | 静默 | 静默 |

系统会根据分类采取以下处理方式。

- **自动分析**会立即调用 AI 适配器
- **操作卡片**允许用户选择“分析”或“跳过”
- **提示**会显示简短说明，用户可以输入斜杠命令开始分析
- **静默**只记录事件，不打断用户

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

1. 收集命令文本、退出码和最多 8 KiB 的输出摘录
2. 构造 prompt 发送到 AI 适配器（cosh-core）
3. 适配器流式返回分析结果
4. cosh-shell 以 Markdown 格式渲染分析内容
5. 用户可在分析过程中 Ctrl+C 取消

## 配置

```toml
[shell]
# 分析模式可选 smart、auto 或 manual
analysis_mode = "smart"
```

运行时也可以切换。

```
/mode analysis smart
/mode analysis auto
/mode analysis manual
```
