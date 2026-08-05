# 会话压缩

[English](../../../../en/user-entrypoint/cosh-ng/shell/session-compaction.md)

会话压缩会缩减发送给模型的对话历史，但不会删除原始 transcript。它适合长期运维
会话，因为其中的命令输出和 Agent 交互会持续积累大量上下文。

## 手动压缩会话

在 Shell 提示符中执行下面的命令。

```text
/session compact
```

手动压缩会立即在后台启动，不受自动 Token 阈值或 `preserve_recent_runs` 限制。
压缩期间仍可使用普通 Shell，但 Agent 请求会暂停，直到压缩结束。

一个已完成的 Agent run 就足够。compactor 会优先选择已经能够满足目标的较早安全
边界，必要时也可以摘要到最新已完成 run。未完成的用户轮次或 tool exchange
不会进入摘要前缀。

任务运行期间可以使用下面的命令。

```text
/session compact status
/session compact cancel
```

取消不会改变完整 transcript 或当前 projection。

## 自动与 Emergency 压缩

每个 Agent run 到达 idle 边界后都会评估自动压缩。默认情况下，模型可见历史超过
可用模型窗口的 70% 时启动，并以不超过 30% 为目标。它会原样保留最近两个完整
Agent run，因此首次自动压缩通常至少需要三个完整 run。

仅超过阈值还不够。如果不存在新的安全前缀，Core 会等待下一个完整 run，而不会
启动一个最终以 `nothing_to_compact` 失败的后台任务。

达到可用窗口的 90% 时，Core 会在下一次 provider 请求前同步执行保护。它先按配置
保留最近 run。当前安全边界腾出的空间仍然不够，Core 会改为只保留一个已完成 run，
再尝试手动压缩使用的安全边界。整个过程中，进行中的 run 都会原样保留。压缩调用
失败或所有安全边界都无法腾出足够空间时，Core 会在发送超大请求前返回类型化的
上下文上限错误。

## 数据与安全保证

- 持久化 transcript 保持完整且只追加。
- 版本化摘要 projection 只改变发送给模型的上下文。
- 压缩不会拆开 tool call 与其结果。
- provider 调用不会持有会话存储锁。
- generation、digest 和 revision 校验会拒绝过期提交。
- 只有确实缩小有效上下文的摘要才会提交。
- provider 失败、取消或无效输出不会改变原 projection。

当不存在完整前缀、单个 run 超过 summarizer 输入预算、认证或 provider 失败、
会话发生并发变化，或者摘要没有缩小上下文时，压缩仍会返回可操作的失败信息。

## 配置

```toml
[session.compaction]
enabled = true
auto = true
trigger_ratio = 0.70
emergency_ratio = 0.90
target_ratio = 0.30
preserve_recent_runs = 2

# 可选的模型级覆盖
# auto_compact_token_limit = 89600
# model_context_window = 128000
# model_max_output_tokens = 8192
```

`preserve_recent_runs` 控制常规自动压缩。Emergency 保护先使用同一个值，再按上文的
顺序逐步放宽保留范围。显式 `/session compact` 直接使用最后一级策略，可以摘要最新
的已完成 run。这些路径都不会摘要进行中的 run。

`model_max_output_tokens` 既是从模型窗口中预留的回复空间，也是发给 provider 的
`max_tokens` 上限。未设置时，已知模型系列取
`min(模型输出能力, 16384)`，未知模型取 `4096`。Core 会把这两个值都限制在已解析
上下文窗口的一半以内。调低该配置会给历史留出更多空间，也会缩短单次回复的最长
长度。完整配置参考见[配置](../configuration.md)。
