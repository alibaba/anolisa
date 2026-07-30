# Tokenless 效果度量

[English](../../../en/token-saving/tokenless/measuring-savings.md)

Tokenless 记录自己处理的 Payload 在压缩前后的大小和估算 Token 数。它回答的是“压缩候选内容缩小了多少”，不是“模型请求或账单减少了多少”。

数据库和 CLI 把大小字段称为“字符”，但当前写入器实际保存 UTF-8 字节长度。统计中的 Token 数使用近似 `ceil(bytes / 4)` 规则，并没有调用模型 Tokenizer。二者都应只作为对比指标。

## 先理解统计范围

Tokenless 可以度量：

- Schema 压缩前后的大小。
- 工具/API 响应压缩前后的大小。
- TOON 编码前后的大小。
- 改写后的 RTK 命令真正执行时，RTK 过滤输出前后的大小。
- active 与 dry-run 模式。
- Session、Agent 和 Tool Use 标识。

Tokenless 不能直接度量：

- 模型生成的输出 Token。
- system prompt 和未经过 Tokenless 的对话历史。
- 提供商最终计费 Token。
- 压缩是否改变了任务质量。
- 追加型 Adapter 是否从最终模型请求中移除了原始结果。

因此上线评估应同时比较统计数据和任务结果质量。

## 查看累计摘要

```bash
tokenless stats summary
```

当前文本输出包含以下结构：

```text
Tokenless Statistics Summary
============================================================
Total Records: ...

Character Savings:
  Before: ...
  After:  ...
  Saved:  ...

Token Savings:
  Before: ...
  After:  ...
  Saved:  ...

Breakdown by Operation:
----------------------------------------
  compress-response: ...
```

输出中的 `Character Savings` 和 `Chars` 是上文说明的、基于字节的兼容字段名。

机器读取使用：

```bash
tokenless stats summary --json
```

摘要默认读取最近最多 10,000 条记录。可限制查询数量：

```bash
tokenless stats summary --limit 1000
```

## 查看单条记录

列出最近记录：

```bash
tokenless stats list
tokenless stats list --limit 50
```

输出中的 `[ID:<n>]` 是记录 ID。查看某次压缩的完整前后文本：

```bash
tokenless stats show <record-id>
```

解释该记录的估算节省和内容变化：

```bash
tokenless stats diff <record-id>
tokenless stats diff <record-id> -U 5
tokenless stats diff <record-id> --json
```

当两端都是合法 JSON 时，`diff` 会在展示前对对象 key 排序，因此只改变 key 顺序的差异不会显示；存储内容本身不会被修改。需要查看原始 Payload，或 diff 提示内容缺失、过大时，使用 `stats show`。

分析一个 Session 内可确认衔接的端到端阶段：

```bash
tokenless stats diff --session <session-id>
tokenless stats diff --session <session-id> --sort time
tokenless stats diff --session <session-id> \
  --tool-use-id <tool-use-id>
```

Session 总览只包含指标。tool-use 报告会显示内容差异；只有 session/tool-use ID 相同，并且上一阶段存储的输出与下一阶段输入完全一致时，连续的 active 阶段才会串联。断开的阶段、dry-run 记录及缺少 tool-use ID 的记录保持独立，避免重复计算中间输入。

对于 dry-run 记录，`after` 表示预测压缩大小，`emitted` 仍是原始 `before` 大小。无估算节省的操作不会入库，因此 Session 报告只覆盖节省记录。

> 本地统计包含完整工具文本。不要把 `stats show` 输出粘贴到公开 Issue、共享日志或不受信任的聊天中。详见[配置与数据隐私](configuration-and-privacy.md)。

## 为什么没有记录

以下情况不会新增统计记录：

- 压缩后估算 Token 数没有下降。
- `stats_enabled=false` 或 `TOKENLESS_STATS_ENABLED=0`。
- Adapter 没有启用或旧 Agent 会话尚未重启。
- Hook/Plugin 无法找到 `tokenless`。
- 输入没有经过 Tokenless 支持的 Hook。

追加型 Adapter 即使让宿主保留了原始结果，也可能产生统计记录。尤其是当前 Codex PostToolUse Hook 只记录压缩候选，不替换原始工具输出。

先运行：

```bash
tokenless stats status
anolisa adapter status tokenless
```

继续参阅[启用后没有产生统计记录](troubleshooting.md#启用后没有产生统计记录)。

## 用 dry-run 做双跑对比

dry-run 会计算压缩结果和预测节省，但向调用方返回原文。要对同一输入做最小可重复对比：

```bash
TOKENLESS_COMPRESSION_ENABLED=0 \
  tokenless compress-response -f response.json \
  --session-id baseline-run

TOKENLESS_COMPRESSION_ENABLED=1 \
  tokenless compress-response -f response.json \
  --session-id active-run

tokenless stats summary --compare baseline-run active-run
```

机器读取：

```bash
tokenless stats summary \
  --compare baseline-run active-run \
  --json
```

注意：

- `--compare` 必须提供恰好两个 Session ID，顺序为 baseline、active。
- baseline 应为 dry-run，active 应为真实压缩；模式不匹配时 CLI 会告警。
- 对真实 Agent 任务做对比时，应尽量使用相同输入、工具版本和环境。
- dry-run 仍会把压缩前后文本写入本地统计数据库。
- dry-run 不创建 Stash 条目，也不会关闭 RTK 重写。RTK 写入的记录没有显式 mode，读取时按 active 处理，因此可能触发基线模式警告。

## 正确解释节省率

`stats summary` 中的压缩率只针对 Tokenless 经手的 Payload。估算会话总体收益时，可以使用：

```text
总体估算节省率
= Tokenless Payload 压缩率 × 工具 Payload 占会话总 Token 的比例
```

例如，Payload 压缩率为 60%，但工具 Payload 只占会话总 Token 的 20%，则总体估算收益约为 12%。这个结果仍不是提供商账单保证值。

## AgentSight 本地展示

AgentSight 的 Token savings 页面可以只读聚合 `~/.tokenless/stats.db`。两者由同一用户运行，且 AgentSight 能访问该数据库时，不需要通过 SLS 才能看到本地 Tokenless 统计。

安装后可先确认：

```bash
test -r ~/.tokenless/stats.db
```

AgentSight 的安装和 Dashboard 使用方式见[AgentSight 用户指南](../../agent-observability/agentsight.md)。

## SLS JSONL

SLS 是独立的外部采集通道，不是 AgentSight 读取本地统计的前置条件。

默认行为：

- `sls_enabled=true`。
- 默认目标为 `/var/log/anolisa/sls/ops/tokenless.jsonl`。
- Tokenless 只在目标文件已经存在时追加；不存在时静默跳过。
- 文件由 ANOLISA SLS/Logtail 设施创建、轮转和删除。
- SLS 记录只包含度量与标识，不包含压缩前后的原文。
- 随包提供的 RTK 统计写入器只把 `rewrite-command` 记录写入本地 SQLite，不会调用 SLS Writer。

自定义测试文件：

```bash
touch /tmp/tokenless-sls.jsonl
TOKENLESS_SLS_ENABLED=1 \
TOKENLESS_SLS_PATH=/tmp/tokenless-sls.jsonl \
  tokenless compress-response -f response.json

tail -n 1 /tmp/tokenless-sls.jsonl | jq .
```

`TOKENLESS_SLS_PATH` 必须位于 `/var/log/` 或 `/tmp/` 下。生产 SLS endpoint、认证和 Logtail 配置属于平台运维配置，不在 Tokenless 用户指南中展开。

## 清理统计

先确认不再需要历史对比：

```bash
tokenless stats clear --yes
```

这会清空统计记录，但不会禁用后续记录。停止新增记录：

```bash
tokenless stats disable
```

`stats disable` 只关闭本地 SQLite 统计，不会关闭 SLS。完整开关关系见[配置与数据隐私](configuration-and-privacy.md)。
