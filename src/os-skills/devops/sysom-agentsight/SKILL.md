---
name: agentsight
description: 通过命令行查询 AgentSight 平台的 token 消耗数据和审计事件。当用户询问 token 用量、花费、消耗趋势,或询问 LLM 调用、进程行为审计时使用此技能。
---

# Token 查询

## 常用命令

| 命令 | 说明 |
|------|------|
| `/usr/local/bin/agentsight token --period today` | 今天消耗 |
| `/usr/local/bin/agentsight token --period yesterday` | 昨天消耗 |
| `/usr/local/bin/agentsight token --hours 3` | 最近 3 小时 |
| `/usr/local/bin/agentsight token --period today --compare` | 今天 vs 昨天对比 |

## 返回示例

```
今天共消耗 125,000 tokens，比昨天（98,000）增长 27%。

输入: 125,000 | 输出: 85,000
```

---

# 审计查询

## 常用命令

| 命令 | 说明 |
|------|------|
| `/usr/local/bin/agentsight audit` | 最近 24 小时事件 |
| `/usr/local/bin/agentsight audit --last 48` | 最近 48 小时 |
| `/usr/local/bin/agentsight audit --pid 12345` | 指定进程 |
| `/usr/local/bin/agentsight audit --type llm` | 仅 LLM 调用 |
| `/usr/local/bin/agentsight audit --type process` | 仅进程行为 |
| `/usr/local/bin/agentsight audit --summary` | 汇总统计 |
| `/usr/local/bin/agentsight audit --summary --last 72` | 最近 72 小时汇总 |
| `/usr/local/bin/agentsight audit --json` | JSON 格式 |

## 返回示例

**汇总输出：**
```
=== Audit Summary (last 24 hours) ===

LLM calls:        42
Process actions:  128

Providers:
  OpenAI: 35 calls
  Anthropic: 7 calls

Top commands:
  python agent.py: 25 times
  node server.js: 17 times
```

**事件列表（JSON）：**
```json
{"event_type":"llm_call","pid":1234,"comm":"python",
 "extra":{"provider":"OpenAI","model":"gpt-4o","input_tokens":1500,"output_tokens":800}}
```

## 事件类型

| 类型 | 字段 |
|------|------|
| `llm_call` | provider, model, input_tokens, output_tokens, request_path, response_status, is_sse |
| `process_action` | filename, args, exit_code |

---

# 注意事项

- 数据存储：`/var/log/sysak/.agentsight/agentsight.db`（SQLite）
- 默认保留：7 天
- 时间戳：纳秒级 Unix 时间戳
- 权限：需要 root 运行 eBPF
