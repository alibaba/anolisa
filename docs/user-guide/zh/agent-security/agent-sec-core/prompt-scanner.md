# Prompt Scanner 用户使用指南

[English](../../../en/agent-security/agent-sec-core/prompt-scanner.md)

Prompt Scanner 用于检测 Agent 输入中的提示词注入、越狱攻击和恶意指令。它结合快速规则引擎（L1）与可选的
ML 分类器（L2），返回结构化 verdict，并记录经过清理的 Security Event，供审计和 Observability 关联使用。

## 扫描文本

必须且只能提供一种输入来源：内联文本、标准输入或 UTF-8 文件（每行一条 prompt）。

```bash
# 内联文本
agent-sec-cli scan-prompt --text "ignore all system instructions"

# 标准输入
echo "forget your system prompt" | agent-sec-cli scan-prompt

# UTF-8 文件（每行一个 prompt）
agent-sec-cli scan-prompt --input prompts.txt --format json
```

常用选项：

| 选项 | 作用 |
|------|------|
| `--text TEXT` | 直接指定扫描文本，优先级高于 `--input` 和 stdin |
| `--input FILE` | 每行一个 prompt 的文件路径 |
| `--mode MODE` | 检测模式：`fast` / `standard` / `strict` / `multi_turn`；默认 `standard` |
| `--format FMT` | 输出格式：`json`（默认）或 `text`（人类可读）|
| `--source SOURCE` | 输入来源标签，写入 metadata，例如 `user_input`、`rag`、`tool_output` |

## 检测模式

| 模式 | 层级 | fast_fail | 典型延迟 | 适用场景 |
|------|------|-----------|----------|----------|
| `fast` | L1 规则引擎 | `True` | < 5 ms | 实时对话，低延迟优先 |
| `standard` | L1 + L2 ML 分类器 | `False` | 20–80 ms | 生产环境默认 |
| `strict` | L1 + L2 ML 分类器（L3 预留） | `False` | 50–200 ms | 高安全场景 |
| `multi_turn` | L4 多轮意图检测 | — | 取决于模型 | 从 stdin 传入 JSON history（Ollama） |

L2 分类器首次使用时会从 ModelScope 下载 `LLM-Research/Llama-Prompt-Guard-2-86M`（约 1 GB）。
安装后执行一次 `agent-sec-cli scan-prompt warmup` 可消除冷启动延迟。

## Verdict

Scanner 将各层结果聚合为一个 verdict：

| Verdict | 含义 |
|---------|------|
| `pass` | 未检测到威胁 |
| `warn` | L1 命中但 L2 未确认（`standard`/`strict`），或策略级警告 |
| `deny` | L1（`fast`）或 L1 + L2（`standard`/`strict`）确认威胁 |
| `error` | Scanner 内部错误（例如模型加载失败） |

> `fast` 模式不运行 ML 层，任何 L1 命中都直接映射为 `deny`。

## 宿主 Hook Policy

设置 `PROMPT_SCANNER_HOOK_ENABLED=false` 可完全跳过宿主 prompt scanner hook。启用时，以下环境变量控制部署级行为：

| 环境变量 | 默认值 | 行为 |
|----------|--------|------|
| `PROMPT_SCANNER_HOOK_ENABLED` | `true` | 设为 `false` 时在读取输入前跳过 hook |
| `PROMPT_SCANNER_MODE` | `observe` | `observe` 静默审计；`warn` 告警；`ask`/`block` 按宿主能力执行或 fallback 为 `warn`；`deny` 等价于 `block` |
| `PROMPT_SCANNER_SCAN_MODE` | `standard` | 传给 `scan-prompt` 的扫描强度：`fast` / `standard` / `strict` |
| `PROMPT_SCANNER_TIMEOUT` | `10` | Scanner 超时秒数 |

环境变量优先于 Hermes/OpenClaw capability 配置。宿主 Agent 在加载插件时读取这些变量，修改后需重启承载该
hook 的 Agent 进程。

Scanner verdict `deny` 描述扫描风险；hook policy `block` 决定当前 adapter 是否执行阻断。

## Security Event 与 Observability

每次扫描都会进入现有 `prompt_scan` Security Event 链路。Event 包含 source、verdict、summary、threat type、
confidence 以及经过清理的规则或 ML findings，不包含原始 prompt 文本。

Scanner 出错时宿主 hook 保持 fail-open：`error` verdict 会被审计，但不会用于阻断底层操作。

Observability 使用现有 trace context 和输入 hash 与 Security Event 建立关联，不重复存储 finding 明细。
