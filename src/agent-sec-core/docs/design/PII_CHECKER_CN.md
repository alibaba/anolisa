# PIIChecker 设计

## 1. 文档定位

PIIChecker 是 `agent-sec-core` 的隐私/敏感信息检测能力，用于识别文本、文件或工具输出中的个人信息、凭据和敏感数据。

## 2. 设计目标

1. 识别常见 PII、凭据和密钥类敏感信息。
2. 默认输出脱敏 evidence，避免在扫描结果中传播原始敏感内容。
3. 提供 CLI、Python API 和安全中间层集成接口。
4. 支持规则、校验器和上下文评分组合，降低误报。

## 3. 检测范围

| 类别 | 示例 |
|---|---|
| 个人身份信息 | 姓名、身份证/证件号、护照号 |
| 联系方式 | 邮箱、手机号、固定电话 |
| 地址信息 | 详细地址、邮编 |
| 金融信息 | 银行卡号、IBAN、信用卡号 |
| 网络身份 | IP、MAC、设备 ID |
| 凭据/密钥 | API key、OAuth token、JWT、私钥片段、SSH key |
| 云厂商凭据 | AK/SK、临时 token、访问密钥 ID |

## 4. 架构

```text
CLI / Python API / security_middleware / hook
        |
        v
PIIChecker
        |
        +--> Preprocessor
        +--> Regex Detector
        +--> Validator
        +--> Context Scorer
        +--> Redactor
        |
        v
PiiScanResult
```

## 5. CLI 设计

```bash
agent-sec-cli scan-pii --text "..."
agent-sec-cli scan-pii --input /path/to/file
agent-sec-cli scan-pii --input /path/to/file --format json
```

参数：

| 参数 | 默认 | 说明 |
|---|---|---|
| `--text` | 无 | 直接扫描文本 |
| `--input` | 无 | 扫描文件 |
| `--format` | `json` | `json` 或 `text` |
| `--include-low-confidence` | false | 是否输出低置信度发现 |
| `--raw-evidence` | false | 是否允许输出原始 evidence，默认禁止 |

## 6. 输出 Schema

默认 JSON 输出：

```jsonc
{
  "ok": true,
  "verdict": "deny",
  "summary": "Detected 2 sensitive item(s), including credential-like data",
  "findings": [
    {
      "type": "email",
      "category": "pii",
      "severity": "warn",
      "confidence": 0.98,
      "evidence_redacted": "te***@example.com",
      "span": { "start": 12, "end": 28 },
      "metadata": {}
    },
    {
      "type": "api_key",
      "category": "credential",
      "severity": "deny",
      "confidence": 0.95,
      "evidence_redacted": "sk-***abcd",
      "span": { "start": 50, "end": 92 },
      "metadata": { "provider": "generic" }
    }
  ],
  "elapsed_ms": 3
}
```

`verdict` 计算：

| findings | verdict |
|---|---|
| 空 | `pass` |
| 仅低/中风险 PII | `warn` |
| 包含凭据、私钥、token | `deny` |
| 内部错误 | `error` |

## 7. 脱敏策略

默认只输出 `evidence_redacted`，不输出原始 evidence。

| 类型 | 脱敏示例 |
|---|---|
| 邮箱 | `te***@example.com` |
| 手机号 | `138****0000` |
| 银行卡 | `6222********1234` |
| API key | `sk-***abcd` |
| 私钥 | `<private-key-redacted>` |

只有显式指定 `--raw-evidence` 且运行环境允许时，才输出原始 evidence。hook、审计日志和报告场景应使用脱敏输出。

## 8. 检测器设计

### 8.1 Regex Detector

用于高确定性模式：

- 邮箱。
- 手机号。
- JWT。
- PEM 私钥头尾。
- 常见 API key 前缀。
- 云厂商 access key。

### 8.2 Validator

用于降低误报：

- 信用卡 Luhn 校验。
- 身份证校验位。
- 日期合法性。
- JWT 三段结构和 base64url 检查。

### 8.3 Context Scorer

结合上下文关键词调整置信度：

| 上下文 | 影响 |
|---|---|
| `password`、`secret`、`token`、`api_key` | 提高凭据置信度 |
| `example`、`dummy`、`test`、`.invalid` | 降低真实泄漏置信度 |
| 代码注释/文档样例 | 降低默认严重度，但不完全忽略 |

## 9. 集成接口

| 接口 | 说明 |
|---|---|
| CLI | `agent-sec-cli scan-pii` |
| Python API | `agent_sec_cli.pii_checker.scan(...)` |
| security_middleware | 作为独立 backend 接入统一调用链 |
| security_events | 记录脱敏后的敏感信息检测事件 |

## 10. 测试与评测

测试重点：

- 每种 detector 的 positive/negative 样本。
- 脱敏结果不泄漏原文。
- validator 降低误报。
- `--raw-evidence` 默认关闭。
- 大文件扫描的性能和内存占用。

测试数据必须使用伪造样本，不允许包含真实用户数据。
